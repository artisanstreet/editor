//! Paired terminal settlement of a running bound run.
//!
//! `Repository::complete_run` and `Repository::fail_run` co-commit the
//! running dispatch, the running assistant run, the final assistant item, the
//! origin turn, the conversation-state counters, and the two lifecycle patches
//! in one `SeaORM` transaction. Every mutable row is fenced on its full snapshot;
//! a zero-row update rolls the whole transaction back with a typed conflict.
//! An exact replay of an identical command is harmless and answers an explicit
//! `Already*` outcome without mutation.

use artisan_domain::{AssistantBody, AssistantMessagePhase, ItemId, PatchId, Revision, UnixMillis};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DbBackend, EntityTrait, Statement, TransactionTrait,
};
use thiserror::Error;

use crate::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, DispatchState,
    EntityLifecycle, RenderPhase,
};

use super::super::message_dispatch::DispatchLeaseOwner;
use super::super::run_launch::stored_bytes_match;
use super::RunBatchScope;
use super::projection;
use crate::repository::{Repository, RepositoryError, corrupt_data, database_error, millis};

// ---------------------------------------------------------------------------
// Bounded run-error wrappers
// ---------------------------------------------------------------------------

const RUN_ERROR_CODE_MIN_BYTES: usize = 1;
const RUN_ERROR_CODE_MAX_BYTES: usize = 128;
const RUN_ERROR_MESSAGE_MIN_BYTES: usize = 1;
const RUN_ERROR_MESSAGE_MAX_BYTES: usize = 1024;

/// Bounded non-empty run error code persisted as `assistant_runs.error_code`.
///
/// Validates `1..=128` UTF-8 bytes without truncation and exposes no
/// formatting that would leak the contained text through `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct RunErrorCode(String);

impl RunErrorCode {
    /// Creates a validated error code.
    ///
    /// # Errors
    ///
    /// Returns a static reason when `value` is empty or exceeds 128 UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, &'static str> {
        let len = value.len();
        if !(RUN_ERROR_CODE_MIN_BYTES..=RUN_ERROR_CODE_MAX_BYTES).contains(&len) {
            return Err("run error code must be 1..=128 bytes");
        }
        if value.is_empty() {
            return Err("run error code must not be empty");
        }
        Ok(Self(value))
    }

    /// Validated code as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RunErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunErrorCode")
            .field("len", &self.0.len())
            .finish()
    }
}

/// Bounded non-empty run error message persisted as `assistant_runs.error_message`.
#[derive(Clone, PartialEq, Eq)]
pub struct RunErrorMessage(String);

impl RunErrorMessage {
    /// Creates a validated error message.
    ///
    /// # Errors
    ///
    /// Returns a static reason when `value` is empty or exceeds 1024 UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, &'static str> {
        let len = value.len();
        if !(RUN_ERROR_MESSAGE_MIN_BYTES..=RUN_ERROR_MESSAGE_MAX_BYTES).contains(&len) {
            return Err("run error message must be 1..=1024 bytes");
        }
        if value.is_empty() {
            return Err("run error message must not be empty");
        }
        Ok(Self(value))
    }

    /// Validated message as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RunErrorMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunErrorMessage")
            .field("len", &self.0.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Borrowed inputs of one atomic completion.
pub struct CompleteRun<'a> {
    /// Full pair snapshot and credentials binding the running run.
    pub scope: RunBatchScope<'a>,
    /// Caller-injected terminal time; no internal clock and no TTL.
    pub operated_at: UnixMillis,
    /// Final assistant item to settle.
    pub item_id: &'a ItemId,
    /// Revision the caller observed on the item; must equal the stored revision.
    pub expected_revision: Revision,
    /// Settled body for the terminal item.
    pub body: &'a AssistantBody,
    /// Settled phase for the terminal item.
    pub phase: AssistantMessagePhase,
    /// Caller-minted `item_lifecycle` patch identity.
    pub item_patch_id: &'a PatchId,
    /// Caller-minted `turn_lifecycle` patch identity.
    pub turn_patch_id: &'a PatchId,
}

/// Borrowed inputs of one atomic failure.
pub struct FailRun<'a> {
    /// Full pair snapshot and credentials binding the running run.
    pub scope: RunBatchScope<'a>,
    /// Caller-injected terminal time.
    pub operated_at: UnixMillis,
    /// Final assistant item to settle.
    pub item_id: &'a ItemId,
    /// Revision the caller observed on the item.
    pub expected_revision: Revision,
    /// Settled body for the terminal item.
    pub body: &'a AssistantBody,
    /// Settled phase for the terminal item.
    pub phase: AssistantMessagePhase,
    /// Caller-minted `item_lifecycle` patch identity.
    pub item_patch_id: &'a PatchId,
    /// Caller-minted `turn_lifecycle` patch identity.
    pub turn_patch_id: &'a PatchId,
    /// Bounded run error code.
    pub error_code: &'a RunErrorCode,
    /// Bounded run error message.
    pub error_message: &'a RunErrorMessage,
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Payload-free durable receipt of one terminal settlement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRunReceipt {
    /// Settled run identity.
    pub run_id: artisan_domain::RunId,
    /// Generation recorded on the run.
    pub generation: i64,
    /// Terminal time recorded on the run and dispatch.
    pub terminal_at: UnixMillis,
}

/// Typed outcome of `complete_run`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompleteRunOutcome {
    /// This transaction completed the run.
    Completed(TerminalRunReceipt),
    /// An earlier identical transaction already completed this run.
    AlreadyCompleted(TerminalRunReceipt),
}

/// Typed outcome of `fail_run`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailRunOutcome {
    /// This transaction failed the run.
    Failed(TerminalRunReceipt),
    /// An earlier identical transaction already failed this run.
    AlreadyFailed(TerminalRunReceipt),
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Capability-specific failures of [`Repository::complete_run`].
#[derive(Debug, Error)]
pub enum CompleteRunError {
    /// The supplied run identity does not exist.
    #[error("run `{run_id}` does not exist")]
    RunNotFound {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// The run is not in its running lifecycle.
    #[error("run `{run_id}` is not in running state")]
    RunNotRunning {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// A start key, capability, generation, or binding metadatum mismatched.
    #[error("run `{run_id}` credential or binding metadata did not match")]
    CredentialMismatch {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// The supplied pair snapshot no longer describes persisted state.
    #[error("claimed dispatch snapshot for `{message_id}` no longer matches")]
    SnapshotMismatch {
        /// Claimed message identity.
        message_id: artisan_domain::MessageId,
    },
    /// A colliding or contradictory identity was supplied.
    #[error("run identity conflict: {reason}")]
    IdentityConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A change target is missing, foreign, or otherwise unusable.
    #[error("target conflict: {reason}")]
    TargetConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// The target item is sealed against further mutations.
    #[error("item `{item_id}` is sealed against further mutations")]
    SealedItem {
        /// Sealed item identity.
        item_id: ItemId,
    },
    /// A patch identity collides within the call or durably.
    #[error("patch identity conflict: {reason}")]
    PatchConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A persisted counter could not advance within its checked range.
    #[error("{counter} counter overflowed at {value}")]
    CounterOverflow {
        /// Counter that could not advance.
        counter: &'static str,
        /// Value at the boundary.
        value: i64,
    },
    /// The supplied terminal body or error value violates its bounds.
    #[error("invalid terminal value: {reason}")]
    InvalidError {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A body would exceed its byte ceiling.
    #[error("assistant body would be {length} UTF-8 bytes; the maximum is {maximum}")]
    BodyTooLong {
        /// Offending length.
        length: usize,
        /// Shared body ceiling.
        maximum: usize,
    },
    /// An existing repository rejection surfaced unchanged.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

/// Capability-specific failures of [`Repository::fail_run`].
#[derive(Debug, Error)]
pub enum FailRunError {
    /// The supplied run identity does not exist.
    #[error("run `{run_id}` does not exist")]
    RunNotFound {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// The run is not in its running lifecycle.
    #[error("run `{run_id}` is not in running state")]
    RunNotRunning {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// A start key, capability, generation, or binding metadatum mismatched.
    #[error("run `{run_id}` credential or binding metadata did not match")]
    CredentialMismatch {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// The supplied pair snapshot no longer describes persisted state.
    #[error("claimed dispatch snapshot for `{message_id}` no longer matches")]
    SnapshotMismatch {
        /// Claimed message identity.
        message_id: artisan_domain::MessageId,
    },
    /// A colliding or contradictory identity was supplied.
    #[error("run identity conflict: {reason}")]
    IdentityConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A change target is missing, foreign, or otherwise unusable.
    #[error("target conflict: {reason}")]
    TargetConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// The target item is sealed against further mutations.
    #[error("item `{item_id}` is sealed against further mutations")]
    SealedItem {
        /// Sealed item identity.
        item_id: ItemId,
    },
    /// A patch identity collides within the call or durably.
    #[error("patch identity conflict: {reason}")]
    PatchConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A persisted counter could not advance within its checked range.
    #[error("{counter} counter overflowed at {value}")]
    CounterOverflow {
        /// Counter that could not advance.
        counter: &'static str,
        /// Value at the boundary.
        value: i64,
    },
    /// The supplied terminal body or error value violates its bounds.
    #[error("invalid terminal value: {reason}")]
    InvalidError {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A body would exceed its byte ceiling.
    #[error("assistant body would be {length} UTF-8 bytes; the maximum is {maximum}")]
    BodyTooLong {
        /// Offending length.
        length: usize,
        /// Shared body ceiling.
        maximum: usize,
    },
    /// An existing repository rejection surfaced unchanged.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

// ---------------------------------------------------------------------------
// SQL fences
// ---------------------------------------------------------------------------

const COMPLETE_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'completed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = NULL,
    updated_at_ms = ?
WHERE message_id = ?
  AND correlation_id = ?
  AND attempt_count = ?
  AND queued_at_ms = ?
  AND available_at_ms = ?
  AND state = 'running'
  AND lease_owner = ?
  AND lease_expires_at_ms = ?
  AND updated_at_ms = ?
  AND lease_expires_at_ms > ?
RETURNING message_id
";

const FAIL_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND correlation_id = ?
  AND attempt_count = ?
  AND queued_at_ms = ?
  AND available_at_ms = ?
  AND state = 'running'
  AND lease_owner = ?
  AND lease_expires_at_ms = ?
  AND updated_at_ms = ?
  AND lease_expires_at_ms > ?
RETURNING message_id
";

const COMPLETE_RUN_SQL: &str = r"
UPDATE assistant_runs
SET lifecycle = 'completed',
    owner = NULL,
    lease = NULL,
    claim_token = NULL,
    error_code = NULL,
    error_message = NULL,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND thread_id = ?
  AND origin_message_id = ?
  AND origin_turn_id = ?
  AND lifecycle = 'running'
  AND generation = ?
  AND run_start_key = ?
  AND owner = ?
  AND lease = ?
  AND claim_token IS NULL
  AND created_at_ms = ?
  AND updated_at_ms = ?
  AND provider_binding_version = ?
  AND provider_binding IS NOT NULL
  AND provider_bound_at_ms = ?
  AND error_code IS NULL
  AND terminal_at_ms IS NULL
RETURNING run_id
";

const FAIL_RUN_SQL: &str = r"
UPDATE assistant_runs
SET lifecycle = 'failed',
    owner = NULL,
    lease = NULL,
    claim_token = NULL,
    error_code = ?,
    error_message = ?,
    terminal_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND thread_id = ?
  AND origin_message_id = ?
  AND origin_turn_id = ?
  AND lifecycle = 'running'
  AND generation = ?
  AND run_start_key = ?
  AND owner = ?
  AND lease = ?
  AND claim_token IS NULL
  AND created_at_ms = ?
  AND updated_at_ms = ?
  AND provider_binding_version = ?
  AND provider_binding IS NOT NULL
  AND provider_bound_at_ms = ?
  AND error_code IS NULL
  AND terminal_at_ms IS NULL
RETURNING run_id
";

// ---------------------------------------------------------------------------
// Shared patch input
// ---------------------------------------------------------------------------

struct TerminalPatchInput<'a> {
    thread_id: &'a str,
    patch_id: &'a str,
    sequence: i64,
    kind: ConversationPatchKind,
    revision: i64,
    recorded_at_ms: i64,
    item_id: Option<&'a str>,
    turn_id: Option<&'a str>,
    lifecycle: Option<EntityLifecycle>,
}

async fn insert_terminal_patch(
    transaction: &sea_orm::DatabaseTransaction,
    input: TerminalPatchInput<'_>,
) -> Result<(), RepositoryError> {
    let patch = entities::conversation_patch::ActiveModel {
        patch_id: sea_orm::ActiveValue::Set(input.patch_id.to_owned()),
        thread_id: sea_orm::ActiveValue::Set(input.thread_id.to_owned()),
        sequence: sea_orm::ActiveValue::Set(input.sequence),
        kind: sea_orm::ActiveValue::Set(input.kind),
        revision: sea_orm::ActiveValue::Set(input.revision),
        recorded_at_ms: sea_orm::ActiveValue::Set(input.recorded_at_ms),
        turn_id: sea_orm::ActiveValue::Set(input.turn_id.map(str::to_owned)),
        item_id: sea_orm::ActiveValue::Set(input.item_id.map(str::to_owned)),
        ordinal: sea_orm::ActiveValue::Set(None),
        lifecycle: sea_orm::ActiveValue::Set(input.lifecycle),
        item_kind: sea_orm::ActiveValue::Set(None),
        run_id: sea_orm::ActiveValue::Set(None),
        phase: sea_orm::ActiveValue::Set(None),
        body: sea_orm::ActiveValue::Set(None),
        fragment: sea_orm::ActiveValue::Set(None),
        entity_created_at_ms: sea_orm::ActiveValue::Set(None),
        entity_updated_at_ms: sea_orm::ActiveValue::Set(None),
    };
    patch
        .insert(transaction)
        .await
        .map_err(|source| database_error("insert terminal patch", source))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Repository impl
// ---------------------------------------------------------------------------

impl Repository {
    /// Atomically completes a running bound run together with its dispatch,
    /// final assistant item, origin turn, and patches.
    ///
    /// One transaction fences the RUNNING dispatch (state `running`, exact
    /// snapshot, `lease_expires_at > operated_at`) to `completed`, fences the
    /// `running` run (exact generation/owner/lease/start key/binding tuple) to
    /// `completed` with cleared owner/lease/claim and `terminal_at`,
    /// settles the final assistant item (`streaming` → `completed` with its
    /// supplied body/phase and `revision + 1`) and the origin turn (`active`
    /// → `completed`), advances `conversation_state` counters with two
    /// contiguous patches (`item_lifecycle`, `turn_lifecycle`), and commits
    /// once. Any zero-row fence, patch collision, or constraint violation
    /// rolls the whole transaction back. An exact replay with identical
    /// `operated_at`, body, phase, revisions, and patch identities answers
    /// [`CompleteRunOutcome::AlreadyCompleted`] without mutation.
    ///
    /// # Errors
    ///
    /// Returns [`CompleteRunError::Repository`] for chronology, lease-expiry,
    /// owner, and dispatch-state mismatches, [`CompleteRunError::SnapshotMismatch`]
    /// for stale snapshots, [`CompleteRunError::CredentialMismatch`] for
    /// generation or capability mismatches, and typed patch/target/counter
    /// conflicts. No variant carries secret bytes.
    pub async fn complete_run(
        &self,
        command: CompleteRun<'_>,
    ) -> Result<CompleteRunOutcome, CompleteRunError> {
        validate_complete(&command)?;
        let transaction = self.database.begin().await.map_err(|source| {
            CompleteRunError::Repository(database_error("begin complete run", source))
        })?;
        match execute_complete(&transaction, &command).await {
            Ok(CompleteExecution::Persisted(receipt)) => {
                transaction.commit().await.map_err(|source| {
                    CompleteRunError::Repository(database_error("commit complete run", source))
                })?;
                Ok(CompleteRunOutcome::Completed(receipt))
            }
            Ok(CompleteExecution::Replay(receipt)) => {
                transaction.rollback().await.map_err(|source| {
                    CompleteRunError::Repository(database_error(
                        "roll back complete run replay",
                        source,
                    ))
                })?;
                Ok(CompleteRunOutcome::AlreadyCompleted(receipt))
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    CompleteRunError::Repository(database_error("roll back complete run", source))
                })?;
                Err(error)
            }
        }
    }

    /// Atomically fails a running bound run together with its dispatch, final
    /// item, turn, and patches.
    ///
    /// The same fencing and replay semantics as [`Self::complete_run`] apply;
    /// the run and its item/turn move to `failed` with the supplied bounded
    /// non-empty error pair, and the dispatch moves to `failed` with the error
    /// message as its last error.
    ///
    /// # Errors
    ///
    /// Returns [`FailRunError`] with the same taxonomy as completion, plus
    /// `InvalidError` for malformed error codes/messages.
    pub async fn fail_run(&self, command: FailRun<'_>) -> Result<FailRunOutcome, FailRunError> {
        validate_fail(&command)?;
        let transaction =
            self.database.begin().await.map_err(|source| {
                FailRunError::Repository(database_error("begin fail run", source))
            })?;
        match execute_fail(&transaction, &command).await {
            Ok(FailExecution::Persisted(receipt)) => {
                transaction.commit().await.map_err(|source| {
                    FailRunError::Repository(database_error("commit fail run", source))
                })?;
                Ok(FailRunOutcome::Failed(receipt))
            }
            Ok(FailExecution::Replay(receipt)) => {
                transaction.rollback().await.map_err(|source| {
                    FailRunError::Repository(database_error("roll back fail run replay", source))
                })?;
                Ok(FailRunOutcome::AlreadyFailed(receipt))
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    FailRunError::Repository(database_error("roll back fail run", source))
                })?;
                Err(error)
            }
        }
    }
}

enum CompleteExecution {
    Persisted(TerminalRunReceipt),
    Replay(TerminalRunReceipt),
}

enum FailExecution {
    Persisted(TerminalRunReceipt),
    Replay(TerminalRunReceipt),
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_complete(command: &CompleteRun<'_>) -> Result<(), CompleteRunError> {
    let scope = &command.scope;
    if scope.launched.generation <= 0
        || scope.launched.generation != scope.bound.generation
        || scope.bound.binding_version <= 0
    {
        return Err(CompleteRunError::CredentialMismatch {
            run_id: scope.launched.run_id.clone(),
        });
    }
    if scope.claimed.attempt_count == 0 || i32::try_from(scope.claimed.attempt_count).is_err() {
        return Err(CompleteRunError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.claimed.message_id != scope.launched.message_id
        || scope.claimed.message_id != scope.bound.message_id
    {
        return Err(CompleteRunError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.launched.run_id != scope.bound.run_id {
        return Err(CompleteRunError::IdentityConflict {
            reason: "launched and bound run identities differ",
        });
    }
    if scope.launched.thread_id != scope.bound.thread_id {
        return Err(CompleteRunError::IdentityConflict {
            reason: "launched and bound thread identities differ",
        });
    }
    if command.item_patch_id.as_str() == command.turn_patch_id.as_str() {
        return Err(CompleteRunError::PatchConflict {
            reason: "item and turn patch identities collide",
        });
    }
    if command.body.as_str().len() > AssistantBody::MAX_BYTES {
        return Err(CompleteRunError::BodyTooLong {
            length: command.body.as_str().len(),
            maximum: AssistantBody::MAX_BYTES,
        });
    }
    if i64::try_from(command.expected_revision.get()).is_err() {
        return Err(CompleteRunError::CounterOverflow {
            counter: "revision",
            value: i64::MAX,
        });
    }
    validate_terminal_chronology(scope, command.operated_at).map_err(|error| match error {
        RepositoryError::InvalidChronology { .. }
        | RepositoryError::DispatchLeaseExpired { .. } => CompleteRunError::Repository(error),
        other => CompleteRunError::Repository(other),
    })?;
    Ok(())
}

fn validate_fail(command: &FailRun<'_>) -> Result<(), FailRunError> {
    let scope = &command.scope;
    if scope.launched.generation <= 0
        || scope.launched.generation != scope.bound.generation
        || scope.bound.binding_version <= 0
    {
        return Err(FailRunError::CredentialMismatch {
            run_id: scope.launched.run_id.clone(),
        });
    }
    if scope.claimed.attempt_count == 0 || i32::try_from(scope.claimed.attempt_count).is_err() {
        return Err(FailRunError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.claimed.message_id != scope.launched.message_id
        || scope.claimed.message_id != scope.bound.message_id
    {
        return Err(FailRunError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.launched.run_id != scope.bound.run_id {
        return Err(FailRunError::IdentityConflict {
            reason: "launched and bound run identities differ",
        });
    }
    if scope.launched.thread_id != scope.bound.thread_id {
        return Err(FailRunError::IdentityConflict {
            reason: "launched and bound thread identities differ",
        });
    }
    if command.item_patch_id.as_str() == command.turn_patch_id.as_str() {
        return Err(FailRunError::PatchConflict {
            reason: "item and turn patch identities collide",
        });
    }
    if command.body.as_str().len() > AssistantBody::MAX_BYTES {
        return Err(FailRunError::BodyTooLong {
            length: command.body.as_str().len(),
            maximum: AssistantBody::MAX_BYTES,
        });
    }
    if i64::try_from(command.expected_revision.get()).is_err() {
        return Err(FailRunError::CounterOverflow {
            counter: "revision",
            value: i64::MAX,
        });
    }
    validate_terminal_chronology(scope, command.operated_at).map_err(|error| match error {
        RepositoryError::InvalidChronology { .. }
        | RepositoryError::DispatchLeaseExpired { .. } => FailRunError::Repository(error),
        other => FailRunError::Repository(other),
    })?;
    Ok(())
}

fn validate_terminal_chronology(
    scope: &RunBatchScope<'_>,
    operated_at: UnixMillis,
) -> Result<(), RepositoryError> {
    let relations = [
        (
            scope.claimed.updated_at,
            scope.expected_launch_at,
            "claimed dispatch updated_at",
            "terminal expected_launch_at",
        ),
        (
            scope.expected_launch_at,
            scope.bound.bound_at,
            "terminal expected_launch_at",
            "provider bound_at",
        ),
        (
            scope.bound.bound_at,
            scope.expected_updated_at,
            "provider bound_at",
            "terminal expected_updated_at",
        ),
        (
            scope.expected_updated_at,
            operated_at,
            "terminal expected_updated_at",
            "terminal operated_at",
        ),
    ];
    for (earlier, later, earlier_field, later_field) in relations {
        if earlier.as_millis() > later.as_millis() {
            return Err(RepositoryError::InvalidChronology {
                earlier_field,
                later_field,
            });
        }
    }
    if scope.claimed.lease_expires_at.as_millis() <= operated_at.as_millis() {
        return Err(RepositoryError::DispatchLeaseExpired {
            message_id: scope.claimed.message_id.clone(),
            lease_expires_at_ms: scope.claimed.lease_expires_at.as_millis(),
            operated_at_ms: operated_at.as_millis(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Execute complete
// ---------------------------------------------------------------------------

async fn execute_complete(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<CompleteExecution, CompleteRunError> {
    let dispatch_fenced = fence_complete_dispatch(transaction, command).await?;
    if !dispatch_fenced {
        let replay = classify_complete_unfenced_dispatch(transaction, command).await?;
        if let Some(receipt) = replay {
            return Ok(CompleteExecution::Replay(receipt));
        }
        return Err(classify_complete_dispatch_failure(transaction, command).await);
    }
    if !fence_complete_run(transaction, command).await? {
        let replay = classify_complete_unfenced_dispatch(transaction, command).await?;
        if let Some(receipt) = replay {
            return Ok(CompleteExecution::Replay(receipt));
        }
        return Err(classify_complete_run_failure(transaction, command).await);
    }
    let context = load_complete_context(transaction, command).await?;
    persist_complete(transaction, command, context).await?;
    Ok(CompleteExecution::Persisted(TerminalRunReceipt {
        run_id: command.scope.launched.run_id.clone(),
        generation: command.scope.launched.generation,
        terminal_at: command.operated_at,
    }))
}

async fn fence_complete_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<bool, CompleteRunError> {
    let claimed = command.scope.claimed;
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        COMPLETE_DISPATCH_SQL,
        [
            millis(command.operated_at).into(),
            claimed.message_id.as_str().into(),
            claimed.correlation_id.as_str().into(),
            i64::from(claimed.attempt_count).into(),
            millis(claimed.queued_at).into(),
            millis(claimed.available_at).into(),
            claimed.owner.to_storage().into(),
            millis(claimed.lease_expires_at).into(),
            millis(command.scope.expected_updated_at).into(),
            millis(command.operated_at).into(),
        ],
    );
    let row = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("fence complete dispatch", source))
        })?;
    Ok(row.is_some())
}

async fn fence_complete_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<bool, CompleteRunError> {
    let scope = &command.scope;
    let launched = scope.launched;
    let (owner_cap, lease_cap, _) = scope.credentials.parts();
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        COMPLETE_RUN_SQL,
        [
            millis(command.operated_at).into(),
            millis(command.operated_at).into(),
            launched.run_id.as_str().into(),
            launched.thread_id.as_str().into(),
            launched.message_id.as_str().into(),
            launched.turn_id.as_str().into(),
            launched.generation.into(),
            scope.run_start_key.expose().to_vec().into(),
            owner_cap.expose().to_vec().into(),
            lease_cap.expose().to_vec().into(),
            millis(scope.expected_launch_at).into(),
            millis(scope.expected_updated_at).into(),
            scope.bound.binding_version.into(),
            millis(scope.bound.bound_at).into(),
        ],
    );
    let row = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("fence complete run", source))
        })?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Execute fail
// ---------------------------------------------------------------------------

async fn execute_fail(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<FailExecution, FailRunError> {
    let dispatch_fenced = fence_fail_dispatch(transaction, command).await?;
    if !dispatch_fenced {
        if let Some(receipt) = classify_fail_unfenced_dispatch(transaction, command).await? {
            return Ok(FailExecution::Replay(receipt));
        }
        return Err(classify_fail_dispatch_failure(transaction, command).await);
    }
    if !fence_fail_run(transaction, command).await? {
        if let Some(receipt) = classify_fail_unfenced_dispatch(transaction, command).await? {
            return Ok(FailExecution::Replay(receipt));
        }
        return Err(classify_fail_run_failure(transaction, command).await);
    }
    let context = load_fail_context(transaction, command).await?;
    persist_fail(transaction, command, context).await?;
    Ok(FailExecution::Persisted(TerminalRunReceipt {
        run_id: command.scope.launched.run_id.clone(),
        generation: command.scope.launched.generation,
        terminal_at: command.operated_at,
    }))
}

async fn fence_fail_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<bool, FailRunError> {
    let claimed = command.scope.claimed;
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        FAIL_DISPATCH_SQL,
        [
            command.error_message.as_str().to_owned().into(),
            millis(command.operated_at).into(),
            claimed.message_id.as_str().into(),
            claimed.correlation_id.as_str().into(),
            i64::from(claimed.attempt_count).into(),
            millis(claimed.queued_at).into(),
            millis(claimed.available_at).into(),
            claimed.owner.to_storage().into(),
            millis(claimed.lease_expires_at).into(),
            millis(command.scope.expected_updated_at).into(),
            millis(command.operated_at).into(),
        ],
    );
    let row = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            FailRunError::Repository(database_error("fence fail dispatch", source))
        })?;
    Ok(row.is_some())
}

async fn fence_fail_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<bool, FailRunError> {
    let scope = &command.scope;
    let launched = scope.launched;
    let (owner_cap, lease_cap, _) = scope.credentials.parts();
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        FAIL_RUN_SQL,
        [
            command.error_code.as_str().to_owned().into(),
            command.error_message.as_str().to_owned().into(),
            millis(command.operated_at).into(),
            millis(command.operated_at).into(),
            launched.run_id.as_str().into(),
            launched.thread_id.as_str().into(),
            launched.message_id.as_str().into(),
            launched.turn_id.as_str().into(),
            launched.generation.into(),
            scope.run_start_key.expose().to_vec().into(),
            owner_cap.expose().to_vec().into(),
            lease_cap.expose().to_vec().into(),
            millis(scope.expected_launch_at).into(),
            millis(scope.expected_updated_at).into(),
            scope.bound.binding_version.into(),
            millis(scope.bound.bound_at).into(),
        ],
    );
    let row = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| FailRunError::Repository(database_error("fence fail run", source)))?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Classification helpers - complete
// ---------------------------------------------------------------------------

async fn classify_complete_unfenced_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<Option<TerminalRunReceipt>, CompleteRunError> {
    let Some(dispatch) = load_complete_dispatch_for_replay(transaction, command).await? else {
        return Ok(None);
    };
    if dispatch.state != DispatchState::Completed
        || dispatch.updated_at_ms != millis(command.operated_at)
        || dispatch.lease_owner.is_some()
        || dispatch.lease_expires_at_ms.is_some()
    {
        return Ok(None);
    }
    let Some(run) = load_complete_run_for_replay(transaction, command).await? else {
        return Ok(None);
    };
    if !is_complete_run_replay(&run, command) {
        return Ok(None);
    }
    let Some(item) = load_complete_item_for_replay(transaction, command).await? else {
        return Ok(None);
    };
    if !is_complete_item_replay(&item, command) {
        return Ok(None);
    }
    let turn = load_complete_turn_for_replay(transaction, command).await?;
    let Some(turn) = turn else {
        return Ok(None);
    };
    if turn.lifecycle != EntityLifecycle::Completed
        || turn.updated_at_ms != millis(command.operated_at)
    {
        return Ok(None);
    }
    if !complete_patches_match(transaction, command).await? {
        return Ok(None);
    }
    Ok(Some(TerminalRunReceipt {
        run_id: command.scope.launched.run_id.clone(),
        generation: command.scope.launched.generation,
        terminal_at: command.operated_at,
    }))
}

async fn load_complete_dispatch_for_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<Option<entities::MessageDispatch>, CompleteRunError> {
    entities::message_dispatch::Entity::find_by_id(command.scope.claimed.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("classify complete dispatch", source))
        })
}

async fn load_complete_run_for_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<Option<entities::AssistantRun>, CompleteRunError> {
    entities::assistant_run::Entity::find_by_id(command.scope.launched.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("classify complete run replay", source))
        })
}

fn is_complete_run_replay(run: &entities::AssistantRun, command: &CompleteRun<'_>) -> bool {
    if run.lifecycle != AssistantRunLifecycle::Completed
        || run.terminal_at_ms != Some(millis(command.operated_at))
        || run.updated_at_ms != millis(command.operated_at)
        || run.owner.is_some()
        || run.lease.is_some()
        || run.claim_token.is_some()
        || run.error_code.is_some()
    {
        return false;
    }
    if !stored_bytes_match(&run.run_start_key, command.scope.run_start_key.expose()) {
        return false;
    }
    true
}

async fn load_complete_item_for_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<Option<entities::ConversationItem>, CompleteRunError> {
    entities::conversation_item::Entity::find_by_id(command.item_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("classify complete item", source))
        })
}

fn is_complete_item_replay(item: &entities::ConversationItem, command: &CompleteRun<'_>) -> bool {
    let expected_rev = i64::try_from(command.expected_revision.get())
        .unwrap_or(i64::MAX)
        .checked_add(1)
        .unwrap_or(-1);
    item.lifecycle == EntityLifecycle::Completed
        && item.body == command.body.as_str()
        && item.phase.as_ref() == Some(&map_phase(command.phase))
        && item.updated_at_ms == millis(command.operated_at)
        && item.revision == expected_rev
}

async fn load_complete_turn_for_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<Option<entities::ConversationTurn>, CompleteRunError> {
    entities::conversation_turn::Entity::find_by_id(command.scope.launched.turn_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("classify complete turn", source))
        })
}

async fn complete_patches_match(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<bool, CompleteRunError> {
    let patch_item =
        entities::conversation_patch::Entity::find_by_id(command.item_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                CompleteRunError::Repository(database_error("classify complete patch", source))
            })?;
    let patch_turn =
        entities::conversation_patch::Entity::find_by_id(command.turn_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                CompleteRunError::Repository(database_error("classify complete turn patch", source))
            })?;
    let Some(patch_item) = patch_item else {
        return Ok(false);
    };
    let Some(patch_turn) = patch_turn else {
        return Ok(false);
    };
    Ok(patch_item.kind == ConversationPatchKind::ItemLifecycle
        && patch_item.lifecycle == Some(EntityLifecycle::Completed)
        && patch_turn.kind == ConversationPatchKind::TurnLifecycle
        && patch_turn.lifecycle == Some(EntityLifecycle::Completed))
}

async fn classify_complete_dispatch_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> CompleteRunError {
    let claimed = command.scope.claimed;
    let operated_at_ms = millis(command.operated_at);
    let dispatch = match entities::message_dispatch::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => {
            return CompleteRunError::Repository(RepositoryError::DispatchNotFound {
                message_id: claimed.message_id.clone(),
            });
        }
        Err(source) => {
            return CompleteRunError::Repository(database_error(
                "classify complete unfenced dispatch",
                source,
            ));
        }
    };
    if dispatch.state != DispatchState::Running {
        return CompleteRunError::Repository(RepositoryError::InvalidDispatchState {
            message_id: claimed.message_id.clone(),
            state: dispatch_state_label(&dispatch.state),
        });
    }
    if let Some(expiry) = dispatch.lease_expires_at_ms
        && expiry <= operated_at_ms
    {
        return CompleteRunError::Repository(RepositoryError::DispatchLeaseExpired {
            message_id: claimed.message_id.clone(),
            lease_expires_at_ms: expiry,
            operated_at_ms,
        });
    }
    let owner_matches = dispatch.lease_owner.as_deref().is_some_and(|owner| {
        DispatchLeaseOwner::from_storage(owner)
            .is_ok_and(|persisted| persisted.constant_time_eq(&claimed.owner))
    });
    if !owner_matches {
        return CompleteRunError::Repository(RepositoryError::DispatchOwnerMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    CompleteRunError::SnapshotMismatch {
        message_id: claimed.message_id.clone(),
    }
}

async fn classify_complete_run_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> CompleteRunError {
    let scope = &command.scope;
    let launched = scope.launched;
    let run = match entities::assistant_run::Entity::find_by_id(launched.run_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return CompleteRunError::RunNotFound {
                run_id: launched.run_id.clone(),
            };
        }
        Err(source) => {
            return CompleteRunError::Repository(database_error(
                "classify complete unfenced run",
                source,
            ));
        }
    };
    if run.lifecycle != AssistantRunLifecycle::Running {
        return CompleteRunError::RunNotRunning {
            run_id: launched.run_id.clone(),
        };
    }
    if run.thread_id != launched.thread_id.as_str()
        || run.origin_message_id != launched.message_id.as_str()
        || run.origin_turn_id != launched.turn_id.as_str()
    {
        return CompleteRunError::IdentityConflict {
            reason: "stored run originates from another thread, message, or turn",
        };
    }
    let (owner_cap, lease_cap, _) = scope.credentials.parts();
    if run.generation != launched.generation
        || !stored_bytes_match(&run.run_start_key, scope.run_start_key.expose())
        || !owner_cap.matches_stored(run.owner.as_ref())
        || !lease_cap.matches_stored(run.lease.as_ref())
        || run.claim_token.is_some()
        || run.provider_binding_version != Some(scope.bound.binding_version)
        || run.provider_binding.is_none()
        || run.provider_bound_at_ms != Some(millis(scope.bound.bound_at))
    {
        return CompleteRunError::CredentialMismatch {
            run_id: launched.run_id.clone(),
        };
    }
    CompleteRunError::SnapshotMismatch {
        message_id: scope.claimed.message_id.clone(),
    }
}

// ---------------------------------------------------------------------------
// Classification helpers - fail
// ---------------------------------------------------------------------------

async fn classify_fail_unfenced_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<Option<TerminalRunReceipt>, FailRunError> {
    let dispatch =
        entities::message_dispatch::Entity::find_by_id(command.scope.claimed.message_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                FailRunError::Repository(database_error("classify fail dispatch", source))
            })?;
    let Some(dispatch) = dispatch else {
        return Ok(None);
    };
    if dispatch.state != DispatchState::Failed {
        return Ok(None);
    }
    if dispatch.updated_at_ms != millis(command.operated_at) {
        return Ok(None);
    }
    if dispatch.lease_owner.is_some() || dispatch.lease_expires_at_ms.is_some() {
        return Ok(None);
    }
    if dispatch.last_error.as_deref() != Some(command.error_message.as_str()) {
        return Ok(None);
    }
    let run = entities::assistant_run::Entity::find_by_id(command.scope.launched.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| FailRunError::Repository(database_error("classify fail run", source)))?;
    let Some(run) = run else {
        return Ok(None);
    };
    if run.lifecycle != AssistantRunLifecycle::Failed {
        return Ok(None);
    }
    if run.terminal_at_ms != Some(millis(command.operated_at))
        || run.updated_at_ms != millis(command.operated_at)
        || run.error_code.as_deref() != Some(command.error_code.as_str())
        || run.error_message.as_deref() != Some(command.error_message.as_str())
    {
        return Ok(None);
    }
    let item = entities::conversation_item::Entity::find_by_id(command.item_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| FailRunError::Repository(database_error("classify fail item", source)))?;
    let Some(item) = item else {
        return Ok(None);
    };
    if item.lifecycle != EntityLifecycle::Failed
        || item.body != command.body.as_str()
        || item.phase.as_ref() != Some(&map_phase(command.phase))
    {
        return Ok(None);
    }
    let turn =
        entities::conversation_turn::Entity::find_by_id(command.scope.launched.turn_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                FailRunError::Repository(database_error("classify fail turn", source))
            })?;
    let Some(turn) = turn else {
        return Ok(None);
    };
    if turn.lifecycle != EntityLifecycle::Failed {
        return Ok(None);
    }
    let patch_item =
        entities::conversation_patch::Entity::find_by_id(command.item_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                FailRunError::Repository(database_error("classify fail patch", source))
            })?;
    let patch_turn =
        entities::conversation_patch::Entity::find_by_id(command.turn_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                FailRunError::Repository(database_error("classify fail turn patch", source))
            })?;
    if patch_item.is_none() || patch_turn.is_none() {
        return Ok(None);
    }
    let patch_item = patch_item.unwrap();
    let patch_turn = patch_turn.unwrap();
    if patch_item.kind != ConversationPatchKind::ItemLifecycle
        || patch_item.lifecycle != Some(EntityLifecycle::Failed)
        || patch_turn.kind != ConversationPatchKind::TurnLifecycle
        || patch_turn.lifecycle != Some(EntityLifecycle::Failed)
    {
        return Ok(None);
    }
    Ok(Some(TerminalRunReceipt {
        run_id: command.scope.launched.run_id.clone(),
        generation: command.scope.launched.generation,
        terminal_at: command.operated_at,
    }))
}

async fn classify_fail_dispatch_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> FailRunError {
    let claimed = command.scope.claimed;
    let operated_at_ms = millis(command.operated_at);
    let dispatch = match entities::message_dispatch::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => {
            return FailRunError::Repository(RepositoryError::DispatchNotFound {
                message_id: claimed.message_id.clone(),
            });
        }
        Err(source) => {
            return FailRunError::Repository(database_error(
                "classify fail unfenced dispatch",
                source,
            ));
        }
    };
    if dispatch.state != DispatchState::Running {
        return FailRunError::Repository(RepositoryError::InvalidDispatchState {
            message_id: claimed.message_id.clone(),
            state: dispatch_state_label(&dispatch.state),
        });
    }
    if let Some(expiry) = dispatch.lease_expires_at_ms
        && expiry <= operated_at_ms
    {
        return FailRunError::Repository(RepositoryError::DispatchLeaseExpired {
            message_id: claimed.message_id.clone(),
            lease_expires_at_ms: expiry,
            operated_at_ms,
        });
    }
    let owner_matches = dispatch.lease_owner.as_deref().is_some_and(|owner| {
        DispatchLeaseOwner::from_storage(owner)
            .is_ok_and(|persisted| persisted.constant_time_eq(&claimed.owner))
    });
    if !owner_matches {
        return FailRunError::Repository(RepositoryError::DispatchOwnerMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    FailRunError::SnapshotMismatch {
        message_id: claimed.message_id.clone(),
    }
}

async fn classify_fail_run_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> FailRunError {
    let scope = &command.scope;
    let launched = scope.launched;
    let run = match entities::assistant_run::Entity::find_by_id(launched.run_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return FailRunError::RunNotFound {
                run_id: launched.run_id.clone(),
            };
        }
        Err(source) => {
            return FailRunError::Repository(database_error("classify fail unfenced run", source));
        }
    };
    if run.lifecycle != AssistantRunLifecycle::Running {
        return FailRunError::RunNotRunning {
            run_id: launched.run_id.clone(),
        };
    }
    if run.thread_id != launched.thread_id.as_str()
        || run.origin_message_id != launched.message_id.as_str()
        || run.origin_turn_id != launched.turn_id.as_str()
    {
        return FailRunError::IdentityConflict {
            reason: "stored run originates from another thread, message, or turn",
        };
    }
    let (owner_cap, lease_cap, _) = scope.credentials.parts();
    if run.generation != launched.generation
        || !stored_bytes_match(&run.run_start_key, scope.run_start_key.expose())
        || !owner_cap.matches_stored(run.owner.as_ref())
        || !lease_cap.matches_stored(run.lease.as_ref())
        || run.claim_token.is_some()
        || run.provider_binding_version != Some(scope.bound.binding_version)
        || run.provider_binding.is_none()
        || run.provider_bound_at_ms != Some(millis(scope.bound.bound_at))
    {
        return FailRunError::CredentialMismatch {
            run_id: launched.run_id.clone(),
        };
    }
    FailRunError::SnapshotMismatch {
        message_id: scope.claimed.message_id.clone(),
    }
}

// ---------------------------------------------------------------------------
// Context loading
// ---------------------------------------------------------------------------

struct CompleteContext {
    state: projection::LoadedState,
    turn: projection::LoadedTurn,
    item: entities::ConversationItem,
}

async fn load_complete_context(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<CompleteContext, CompleteRunError> {
    let state = load_complete_state(transaction, command).await?;
    let turn = load_complete_turn(transaction, command, &state).await?;
    let item = load_complete_item(transaction, command, &state, &turn).await?;
    Ok(CompleteContext { state, turn, item })
}

async fn load_complete_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<projection::LoadedState, CompleteRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let state = projection::load_conversation_state(transaction, thread_id)
        .await
        .map_err(map_projection_error_complete)?
        .ok_or_else(|| {
            CompleteRunError::Repository(corrupt_data(
                "conversation_state",
                "thread_id",
                "fenced terminal found no conversation state",
            ))
        })?;
    if state.next_renderer_ordinal < 0 {
        return Err(CompleteRunError::Repository(corrupt_data(
            "conversation_state",
            "next_renderer_ordinal",
            "counter is negative",
        )));
    }
    if state.last_patch_sequence < 0 {
        return Err(CompleteRunError::Repository(corrupt_data(
            "conversation_state",
            "last_patch_sequence",
            "counter is negative",
        )));
    }
    if operated_at_ms < state.updated_at_ms {
        return Err(CompleteRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_state.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if state.last_patch_sequence.checked_add(2).is_none() {
        return Err(CompleteRunError::CounterOverflow {
            counter: "patch sequence",
            value: state.last_patch_sequence,
        });
    }
    Ok(state)
}

async fn load_complete_turn(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    state: &projection::LoadedState,
) -> Result<projection::LoadedTurn, CompleteRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let _ = state;
    let turn = projection::load_turn(transaction, command.scope.launched.turn_id.as_str())
        .await
        .map_err(map_projection_error_complete)?
        .ok_or_else(|| {
            CompleteRunError::Repository(corrupt_data(
                "conversation_turns",
                "turn_id",
                "fenced terminal lost its origin turn",
            ))
        })?;
    if turn.thread_id != thread_id {
        return Err(CompleteRunError::Repository(corrupt_data(
            "conversation_turns",
            "thread_id",
            "origin turn belongs to another thread",
        )));
    }
    if operated_at_ms < turn.updated_at_ms {
        return Err(CompleteRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_turns.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if turn.lifecycle == EntityLifecycle::Completed
        || turn.lifecycle == EntityLifecycle::Failed
        || turn.lifecycle == EntityLifecycle::Cancelled
    {
        return Err(CompleteRunError::TargetConflict {
            reason: "origin turn is sealed",
        });
    }
    Ok(turn)
}

async fn load_complete_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    _state: &projection::LoadedState,
    turn: &projection::LoadedTurn,
) -> Result<entities::ConversationItem, CompleteRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let _ = turn;
    let item = projection::load_item(transaction, command.item_id.as_str())
        .await
        .map_err(map_projection_error_complete)?
        .ok_or_else(|| CompleteRunError::TargetConflict {
            reason: "target item does not exist",
        })?;
    if item.thread_id != thread_id
        || item.turn_id != command.scope.launched.turn_id.as_str()
        || item.run_id.as_deref() != Some(command.scope.launched.run_id.as_str())
        || item.item_kind != ConversationItemKind::AssistantMessage
    {
        return Err(CompleteRunError::TargetConflict {
            reason: "target item belongs to another run, turn, or thread",
        });
    }
    if item.lifecycle == EntityLifecycle::Completed
        || item.lifecycle == EntityLifecycle::Failed
        || item.lifecycle == EntityLifecycle::Cancelled
    {
        return Err(CompleteRunError::SealedItem {
            item_id: command.item_id.clone(),
        });
    }
    if operated_at_ms < item.updated_at_ms {
        return Err(CompleteRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_items.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    let stored_rev = u64::try_from(item.revision).map_err(|_| {
        CompleteRunError::Repository(corrupt_data(
            "conversation_items",
            "revision",
            "revision is negative",
        ))
    })?;
    if stored_rev != command.expected_revision.get() {
        return Err(CompleteRunError::TargetConflict {
            reason: "expected revision does not match the stored item",
        });
    }
    ensure_complete_patches_vacant(transaction, command).await?;
    Ok(item)
}

async fn ensure_complete_patches_vacant(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
) -> Result<(), CompleteRunError> {
    if projection::patch_exists(transaction, command.item_patch_id.as_str())
        .await
        .map_err(map_projection_error_complete)?
    {
        return Err(CompleteRunError::PatchConflict {
            reason: "item patch identity already exists",
        });
    }
    if projection::patch_exists(transaction, command.turn_patch_id.as_str())
        .await
        .map_err(map_projection_error_complete)?
    {
        return Err(CompleteRunError::PatchConflict {
            reason: "turn patch identity already exists",
        });
    }
    Ok(())
}

fn map_projection_error_complete(error: super::RunObservationError) -> CompleteRunError {
    match error {
        super::RunObservationError::Repository(inner) => CompleteRunError::Repository(inner),
        other => CompleteRunError::Repository(RepositoryError::Database {
            operation: "load terminal context",
            source: sea_orm::DbErr::Custom(format!("{other:?}")),
        }),
    }
}

struct FailContext {
    state: projection::LoadedState,
    turn: projection::LoadedTurn,
    item: entities::ConversationItem,
}

async fn load_fail_context(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<FailContext, FailRunError> {
    let state = load_fail_state(transaction, command).await?;
    let turn = load_fail_turn(transaction, command, &state).await?;
    let item = load_fail_item(transaction, command, &state, &turn).await?;
    Ok(FailContext { state, turn, item })
}

async fn load_fail_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<projection::LoadedState, FailRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let state = projection::load_conversation_state(transaction, thread_id)
        .await
        .map_err(map_projection_error_fail)?
        .ok_or_else(|| {
            FailRunError::Repository(corrupt_data(
                "conversation_state",
                "thread_id",
                "fenced terminal found no conversation state",
            ))
        })?;
    if state.next_renderer_ordinal < 0 || state.last_patch_sequence < 0 {
        return Err(FailRunError::Repository(corrupt_data(
            "conversation_state",
            "counter",
            "counter is negative",
        )));
    }
    if operated_at_ms < state.updated_at_ms {
        return Err(FailRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_state.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if state.last_patch_sequence.checked_add(2).is_none() {
        return Err(FailRunError::CounterOverflow {
            counter: "patch sequence",
            value: state.last_patch_sequence,
        });
    }
    Ok(state)
}

async fn load_fail_turn(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    state: &projection::LoadedState,
) -> Result<projection::LoadedTurn, FailRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let _ = state;
    let turn = projection::load_turn(transaction, command.scope.launched.turn_id.as_str())
        .await
        .map_err(map_projection_error_fail)?
        .ok_or_else(|| {
            FailRunError::Repository(corrupt_data(
                "conversation_turns",
                "turn_id",
                "fenced terminal lost its origin turn",
            ))
        })?;
    if turn.thread_id != thread_id {
        return Err(FailRunError::Repository(corrupt_data(
            "conversation_turns",
            "thread_id",
            "origin turn belongs to another thread",
        )));
    }
    if operated_at_ms < turn.updated_at_ms {
        return Err(FailRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_turns.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if matches!(
        turn.lifecycle,
        EntityLifecycle::Completed | EntityLifecycle::Failed | EntityLifecycle::Cancelled
    ) {
        return Err(FailRunError::TargetConflict {
            reason: "origin turn is sealed",
        });
    }
    Ok(turn)
}

async fn load_fail_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    _state: &projection::LoadedState,
    _turn: &projection::LoadedTurn,
) -> Result<entities::ConversationItem, FailRunError> {
    let operated_at_ms = millis(command.operated_at);
    let thread_id = command.scope.launched.thread_id.as_str();
    let item = projection::load_item(transaction, command.item_id.as_str())
        .await
        .map_err(map_projection_error_fail)?
        .ok_or_else(|| FailRunError::TargetConflict {
            reason: "target item does not exist",
        })?;
    if item.thread_id != thread_id
        || item.turn_id != command.scope.launched.turn_id.as_str()
        || item.run_id.as_deref() != Some(command.scope.launched.run_id.as_str())
        || item.item_kind != ConversationItemKind::AssistantMessage
    {
        return Err(FailRunError::TargetConflict {
            reason: "target item belongs to another run, turn, or thread",
        });
    }
    if matches!(
        item.lifecycle,
        EntityLifecycle::Completed | EntityLifecycle::Failed | EntityLifecycle::Cancelled
    ) {
        return Err(FailRunError::SealedItem {
            item_id: command.item_id.clone(),
        });
    }
    if operated_at_ms < item.updated_at_ms {
        return Err(FailRunError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_items.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    let stored_rev = u64::try_from(item.revision).map_err(|_| {
        FailRunError::Repository(corrupt_data(
            "conversation_items",
            "revision",
            "revision is negative",
        ))
    })?;
    if stored_rev != command.expected_revision.get() {
        return Err(FailRunError::TargetConflict {
            reason: "expected revision does not match the stored item",
        });
    }
    ensure_fail_patches_vacant(transaction, command).await?;
    Ok(item)
}

async fn ensure_fail_patches_vacant(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
) -> Result<(), FailRunError> {
    if projection::patch_exists(transaction, command.item_patch_id.as_str())
        .await
        .map_err(map_projection_error_fail)?
    {
        return Err(FailRunError::PatchConflict {
            reason: "item patch identity already exists",
        });
    }
    if projection::patch_exists(transaction, command.turn_patch_id.as_str())
        .await
        .map_err(map_projection_error_fail)?
    {
        return Err(FailRunError::PatchConflict {
            reason: "turn patch identity already exists",
        });
    }
    Ok(())
}

fn map_projection_error_fail(error: super::RunObservationError) -> FailRunError {
    match error {
        super::RunObservationError::Repository(inner) => FailRunError::Repository(inner),
        other => FailRunError::Repository(RepositoryError::Database {
            operation: "load terminal context",
            source: sea_orm::DbErr::Custom(format!("{other:?}")),
        }),
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

async fn persist_complete(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    context: CompleteContext,
) -> Result<(), CompleteRunError> {
    let operated_at_ms = millis(command.operated_at);
    let item_revision = next_revision_complete("conversation_items", context.item.revision)?;
    let turn_revision = next_revision_complete("conversation_turns", context.turn.revision)?;
    let (first_sequence, second_sequence) = terminal_sequences(&context.state)?;
    update_complete_item(
        transaction,
        command,
        &context,
        item_revision,
        operated_at_ms,
    )
    .await?;
    update_complete_turn(transaction, &context, turn_revision, operated_at_ms).await?;
    insert_complete_patches(
        transaction,
        command,
        &context,
        item_revision,
        turn_revision,
        (first_sequence, second_sequence),
        operated_at_ms,
    )
    .await?;
    advance_complete_state(
        transaction,
        command,
        &context,
        second_sequence,
        operated_at_ms,
    )
    .await
}

fn terminal_sequences(state: &projection::LoadedState) -> Result<(i64, i64), CompleteRunError> {
    let first =
        state
            .last_patch_sequence
            .checked_add(1)
            .ok_or(CompleteRunError::CounterOverflow {
                counter: "patch sequence",
                value: state.last_patch_sequence,
            })?;
    let second = first
        .checked_add(1)
        .ok_or(CompleteRunError::CounterOverflow {
            counter: "patch sequence",
            value: first,
        })?;
    Ok((first, second))
}

async fn update_complete_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    context: &CompleteContext,
    item_revision: i64,
    operated_at_ms: i64,
) -> Result<(), CompleteRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_items
SET lifecycle = 'completed',
    body = ?,
    phase = ?,
    revision = ?,
    updated_at_ms = ?
WHERE item_id = ?
  AND revision = ?
RETURNING item_id
",
        [
            command.body.as_str().to_owned().into(),
            render_phase_label(&map_phase(command.phase)).into(),
            item_revision.into(),
            operated_at_ms.into(),
            command.item_id.as_str().into(),
            context.item.revision.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("update terminal item", source))
        })?;
    if updated.is_none() {
        return Err(CompleteRunError::TargetConflict {
            reason: "item fence failed",
        });
    }
    Ok(())
}

async fn update_complete_turn(
    transaction: &sea_orm::DatabaseTransaction,
    context: &CompleteContext,
    turn_revision: i64,
    operated_at_ms: i64,
) -> Result<(), CompleteRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_turns
SET lifecycle = 'completed',
    revision = ?,
    updated_at_ms = ?
WHERE turn_id = ?
  AND revision = ?
RETURNING turn_id
",
        [
            turn_revision.into(),
            operated_at_ms.into(),
            context.turn.turn_id.clone().into(),
            context.turn.revision.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("update terminal turn", source))
        })?;
    if updated.is_none() {
        return Err(CompleteRunError::TargetConflict {
            reason: "turn fence failed",
        });
    }
    Ok(())
}

async fn insert_complete_patches(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    context: &CompleteContext,
    item_revision: i64,
    turn_revision: i64,
    sequences: (i64, i64),
    operated_at_ms: i64,
) -> Result<(), CompleteRunError> {
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope.launched.thread_id.as_str(),
            patch_id: command.item_patch_id.as_str(),
            sequence: sequences.0,
            kind: ConversationPatchKind::ItemLifecycle,
            revision: item_revision,
            recorded_at_ms: operated_at_ms,
            item_id: Some(command.item_id.as_str()),
            turn_id: None,
            lifecycle: Some(EntityLifecycle::Completed),
        },
    )
    .await
    .map_err(CompleteRunError::Repository)?;
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope.launched.thread_id.as_str(),
            patch_id: command.turn_patch_id.as_str(),
            sequence: sequences.1,
            kind: ConversationPatchKind::TurnLifecycle,
            revision: turn_revision,
            recorded_at_ms: operated_at_ms,
            item_id: None,
            turn_id: Some(context.turn.turn_id.as_str()),
            lifecycle: Some(EntityLifecycle::Completed),
        },
    )
    .await
    .map_err(CompleteRunError::Repository)?;
    Ok(())
}

async fn advance_complete_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CompleteRun<'_>,
    context: &CompleteContext,
    second_sequence: i64,
    operated_at_ms: i64,
) -> Result<(), CompleteRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_state
SET last_patch_sequence = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND last_patch_sequence = ?
RETURNING thread_id
",
        [
            second_sequence.into(),
            operated_at_ms.into(),
            command.scope.launched.thread_id.as_str().into(),
            context.state.last_patch_sequence.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            CompleteRunError::Repository(database_error("advance terminal state", source))
        })?;
    if updated.is_none() {
        return Err(CompleteRunError::SnapshotMismatch {
            message_id: command.scope.claimed.message_id.clone(),
        });
    }
    Ok(())
}

async fn persist_fail(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    context: FailContext,
) -> Result<(), FailRunError> {
    let operated_at_ms = millis(command.operated_at);
    let item_revision = next_revision_fail("conversation_items", context.item.revision)?;
    let turn_revision = next_revision_fail("conversation_turns", context.turn.revision)?;
    let (first_sequence, second_sequence) = fail_sequences(&context.state)?;
    update_fail_item(
        transaction,
        command,
        &context,
        item_revision,
        operated_at_ms,
    )
    .await?;
    update_fail_turn(transaction, &context, turn_revision, operated_at_ms).await?;
    insert_fail_patches(
        transaction,
        command,
        &context,
        item_revision,
        turn_revision,
        (first_sequence, second_sequence),
        operated_at_ms,
    )
    .await?;
    advance_fail_state(
        transaction,
        command,
        &context,
        second_sequence,
        operated_at_ms,
    )
    .await
}

fn fail_sequences(state: &projection::LoadedState) -> Result<(i64, i64), FailRunError> {
    let first = state
        .last_patch_sequence
        .checked_add(1)
        .ok_or(FailRunError::CounterOverflow {
            counter: "patch sequence",
            value: state.last_patch_sequence,
        })?;
    let second = first.checked_add(1).ok_or(FailRunError::CounterOverflow {
        counter: "patch sequence",
        value: first,
    })?;
    Ok((first, second))
}

async fn update_fail_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    context: &FailContext,
    item_revision: i64,
    operated_at_ms: i64,
) -> Result<(), FailRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_items
SET lifecycle = 'failed',
    body = ?,
    phase = ?,
    revision = ?,
    updated_at_ms = ?
WHERE item_id = ?
  AND revision = ?
RETURNING item_id
",
        [
            command.body.as_str().to_owned().into(),
            render_phase_label(&map_phase(command.phase)).into(),
            item_revision.into(),
            operated_at_ms.into(),
            command.item_id.as_str().into(),
            context.item.revision.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| FailRunError::Repository(database_error("update failed item", source)))?;
    if updated.is_none() {
        return Err(FailRunError::TargetConflict {
            reason: "item fence failed",
        });
    }
    Ok(())
}

async fn update_fail_turn(
    transaction: &sea_orm::DatabaseTransaction,
    context: &FailContext,
    turn_revision: i64,
    operated_at_ms: i64,
) -> Result<(), FailRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_turns
SET lifecycle = 'failed',
    revision = ?,
    updated_at_ms = ?
WHERE turn_id = ?
  AND revision = ?
RETURNING turn_id
",
        [
            turn_revision.into(),
            operated_at_ms.into(),
            context.turn.turn_id.clone().into(),
            context.turn.revision.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| FailRunError::Repository(database_error("update failed turn", source)))?;
    if updated.is_none() {
        return Err(FailRunError::TargetConflict {
            reason: "turn fence failed",
        });
    }
    Ok(())
}

async fn insert_fail_patches(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    context: &FailContext,
    item_revision: i64,
    turn_revision: i64,
    sequences: (i64, i64),
    operated_at_ms: i64,
) -> Result<(), FailRunError> {
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope.launched.thread_id.as_str(),
            patch_id: command.item_patch_id.as_str(),
            sequence: sequences.0,
            kind: ConversationPatchKind::ItemLifecycle,
            revision: item_revision,
            recorded_at_ms: operated_at_ms,
            item_id: Some(command.item_id.as_str()),
            turn_id: None,
            lifecycle: Some(EntityLifecycle::Failed),
        },
    )
    .await
    .map_err(FailRunError::Repository)?;
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope.launched.thread_id.as_str(),
            patch_id: command.turn_patch_id.as_str(),
            sequence: sequences.1,
            kind: ConversationPatchKind::TurnLifecycle,
            revision: turn_revision,
            recorded_at_ms: operated_at_ms,
            item_id: None,
            turn_id: Some(context.turn.turn_id.as_str()),
            lifecycle: Some(EntityLifecycle::Failed),
        },
    )
    .await
    .map_err(FailRunError::Repository)?;
    Ok(())
}

async fn advance_fail_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailRun<'_>,
    context: &FailContext,
    second_sequence: i64,
    operated_at_ms: i64,
) -> Result<(), FailRunError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r"
UPDATE conversation_state
SET last_patch_sequence = ?,
    updated_at_ms = ?
WHERE thread_id = ?
  AND last_patch_sequence = ?
RETURNING thread_id
",
        [
            second_sequence.into(),
            operated_at_ms.into(),
            command.scope.launched.thread_id.as_str().into(),
            context.state.last_patch_sequence.into(),
        ],
    );
    let updated = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            FailRunError::Repository(database_error("advance failed state", source))
        })?;
    if updated.is_none() {
        return Err(FailRunError::SnapshotMismatch {
            message_id: command.scope.claimed.message_id.clone(),
        });
    }
    Ok(())
}

fn next_revision_complete(table: &'static str, current: i64) -> Result<i64, CompleteRunError> {
    let domain = u64::try_from(current).map_err(|_| {
        CompleteRunError::Repository(corrupt_data(table, "revision", "revision is negative"))
    })?;
    let advanced = artisan_domain::Revision::new(domain)
        .checked_next()
        .map_err(|_| CompleteRunError::CounterOverflow {
            counter: "revision",
            value: current,
        })?;
    i64::try_from(advanced.get()).map_err(|_| CompleteRunError::CounterOverflow {
        counter: "revision",
        value: current,
    })
}

fn next_revision_fail(table: &'static str, current: i64) -> Result<i64, FailRunError> {
    let domain = u64::try_from(current).map_err(|_| {
        FailRunError::Repository(corrupt_data(table, "revision", "revision is negative"))
    })?;
    let advanced = artisan_domain::Revision::new(domain)
        .checked_next()
        .map_err(|_| FailRunError::CounterOverflow {
            counter: "revision",
            value: current,
        })?;
    i64::try_from(advanced.get()).map_err(|_| FailRunError::CounterOverflow {
        counter: "revision",
        value: current,
    })
}

fn map_phase(phase: AssistantMessagePhase) -> RenderPhase {
    match phase {
        AssistantMessagePhase::Unspecified => RenderPhase::Unspecified,
        AssistantMessagePhase::Commentary => RenderPhase::Commentary,
        AssistantMessagePhase::Final => RenderPhase::Final,
    }
}

fn render_phase_label(phase: &RenderPhase) -> &'static str {
    match phase {
        RenderPhase::Unspecified => "unspecified",
        RenderPhase::Commentary => "commentary",
        RenderPhase::Final => "final",
    }
}

const fn dispatch_state_label(state: &DispatchState) -> &'static str {
    match state {
        DispatchState::Queued => "queued",
        DispatchState::Leased => "leased",
        DispatchState::Running => "running",
        DispatchState::Completed => "completed",
        DispatchState::Failed => "failed",
    }
}
