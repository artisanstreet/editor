//! Cancellation and interruption of running bound runs.

use artisan_domain::{AssistantBody, AssistantMessagePhase, ItemId, PatchId, Revision, UnixMillis};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement};
use thiserror::Error;

use crate::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, DispatchState,
    EntityLifecycle,
};
use crate::repository::message_dispatch::DispatchLeaseOwner;
use crate::repository::run_launch::stored_bytes_match;
use crate::repository::run_observation::RunBatchScope;
use crate::repository::run_observation::projection;
use crate::repository::{Repository, RepositoryError, corrupt_data, database_error, millis};

use super::{
    RunErrorCode, RunErrorMessage, TerminalPatchInput, TerminalRunReceipt, dispatch_state_label,
    insert_terminal_patch, map_phase, render_phase_label, validate_terminal_chronology,
};

// ---------------------------------------------------------------------------
// Cancellation and interruption
// ---------------------------------------------------------------------------

/// Borrowed inputs of one atomic cancellation. Cancellation has its own
/// lifecycle and transaction; it is deliberately not represented as a failed
/// run and never calls [`Repository::fail_run`].
pub struct CancelRun<'a> {
    /// Full pair snapshot and credentials binding the running run.
    pub scope: RunBatchScope<'a>,
    /// Caller-injected cancellation time.
    pub operated_at: UnixMillis,
    /// Final assistant item to settle.
    pub item_id: &'a ItemId,
    /// Revision the caller observed on the item.
    pub expected_revision: Revision,
    /// Settled body for the terminal item.
    pub body: &'a AssistantBody,
    /// Settled phase for the terminal item.
    pub phase: AssistantMessagePhase,
    /// Caller-minted item lifecycle patch identity.
    pub item_patch_id: &'a PatchId,
    /// Caller-minted turn lifecycle patch identity.
    pub turn_patch_id: &'a PatchId,
}

/// Borrowed inputs of one atomic interruption. Interruption preserves a
/// non-terminal `terminal_at` and records a bounded error pair required by the
/// durable schema.
pub struct InterruptRun<'a> {
    /// Full pair snapshot and credentials binding the running run.
    pub scope: RunBatchScope<'a>,
    /// Caller-injected interruption time.
    pub operated_at: UnixMillis,
    /// Final assistant item to settle.
    pub item_id: &'a ItemId,
    /// Revision the caller observed on the item.
    pub expected_revision: Revision,
    /// Settled body for the terminal item.
    pub body: &'a AssistantBody,
    /// Settled phase for the terminal item.
    pub phase: AssistantMessagePhase,
    /// Caller-minted item lifecycle patch identity.
    pub item_patch_id: &'a PatchId,
    /// Caller-minted turn lifecycle patch identity.
    pub turn_patch_id: &'a PatchId,
    /// Bounded interruption code.
    pub error_code: &'a RunErrorCode,
    /// Bounded interruption message.
    pub error_message: &'a RunErrorMessage,
}

/// Payload-free receipt of an interruption. The run remains non-terminal from
/// the provider's point of view even though this execution attempt settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterruptedRunReceipt {
    /// Interrupted run identity.
    pub run_id: artisan_domain::RunId,
    /// Generation recorded on the run.
    pub generation: i64,
    /// Time the interruption was recorded.
    pub interrupted_at: UnixMillis,
}

/// Typed outcome of [`Repository::cancel_run`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelRunOutcome {
    /// This transaction cancelled the run.
    Cancelled(TerminalRunReceipt),
    /// An identical cancellation was already durably applied.
    AlreadyCancelled(TerminalRunReceipt),
}

/// Typed outcome of [`Repository::interrupt_run`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterruptRunOutcome {
    /// This transaction recorded the interruption.
    Interrupted(InterruptedRunReceipt),
    /// An identical interruption was already durably applied.
    AlreadyInterrupted(InterruptedRunReceipt),
}

/// Shared typed failures for the two auxiliary terminal capabilities.
///
/// The taxonomy mirrors the existing terminal fences while keeping
/// cancellation and interruption on their own methods. No variant contains a
/// provider payload or a secret capability.
#[derive(Debug, Error)]
pub enum AuxiliaryTerminalError {
    /// The supplied run identity does not exist.
    #[error("run `{run_id}` does not exist")]
    RunNotFound {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// The run is no longer in the running lifecycle.
    #[error("run `{run_id}` is not in running state")]
    RunNotRunning {
        /// Supplied run identity.
        run_id: artisan_domain::RunId,
    },
    /// A credential, generation, or binding metadatum mismatched.
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
    /// A target row is missing, foreign, or unusable.
    #[error("target conflict: {reason}")]
    TargetConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A target item is already sealed.
    #[error("item `{item_id}` is sealed against further mutations")]
    SealedItem {
        /// Sealed item identity.
        item_id: ItemId,
    },
    /// A patch identity collides.
    #[error("patch identity conflict: {reason}")]
    PatchConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A counter could not advance.
    #[error("{counter} counter overflowed at {value}")]
    CounterOverflow {
        /// Counter name.
        counter: &'static str,
        /// Boundary value.
        value: i64,
    },
    /// A terminal value was malformed.
    #[error("invalid terminal value: {reason}")]
    InvalidError {
        /// Bounded reason label.
        reason: &'static str,
    },
    /// The assistant body exceeded the shared domain ceiling.
    #[error("assistant body would be {length} UTF-8 bytes; the maximum is {maximum}")]
    BodyTooLong {
        /// Offending length.
        length: usize,
        /// Shared maximum.
        maximum: usize,
    },
    /// An existing repository rejection surfaced unchanged.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

/// Error alias for cancellation callers.
pub type CancelRunError = AuxiliaryTerminalError;

/// Error alias for interruption callers.
pub type InterruptRunError = AuxiliaryTerminalError;

enum AuxiliaryTerminal<'a> {
    Cancel(&'a CancelRun<'a>),
    Interrupt(&'a InterruptRun<'a>),
}

enum AuxiliaryExecution {
    Persisted(AuxiliaryReceipt),
    Replay(AuxiliaryReceipt),
}

enum AuxiliaryReceipt {
    Cancelled(TerminalRunReceipt),
    Interrupted(InterruptedRunReceipt),
}

struct AuxiliaryContext {
    state: projection::LoadedState,
    turn: projection::LoadedTurn,
    item: entities::ConversationItem,
}

const AUX_CANCEL_DISPATCH_ERROR: &str = "run cancelled";

const AUX_DISPATCH_SQL: &str = r"
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

const AUX_RUN_SQL: &str = r"
UPDATE assistant_runs
SET lifecycle = ?,
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
  AND error_message IS NULL
  AND terminal_at_ms IS NULL
RETURNING run_id
";

impl Repository {
    /// Atomically settles a running bound run as cancelled. Dispatch,
    /// assistant run, item, turn, counters, and lifecycle patches commit as
    /// one transaction; an exact replay returns `AlreadyCancelled`.
    ///
    /// # Errors
    ///
    /// Returns [`CancelRunError`] if validation fails, a required database
    /// operation fails, the fenced cancellation cannot be applied, or the
    /// transaction produces an invalid auxiliary outcome.
    pub async fn cancel_run(
        &self,
        command: CancelRun<'_>,
    ) -> Result<CancelRunOutcome, CancelRunError> {
        let auxiliary = AuxiliaryTerminal::Cancel(&command);
        validate_auxiliary(&auxiliary)?;
        let transaction = self.begin_write().await.map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error("begin cancel run", source))
        })?;
        match execute_auxiliary(&transaction, &auxiliary).await {
            Ok(AuxiliaryExecution::Persisted(AuxiliaryReceipt::Cancelled(receipt))) => {
                transaction.commit().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error("commit cancel run", source))
                })?;
                Ok(CancelRunOutcome::Cancelled(receipt))
            }
            Ok(AuxiliaryExecution::Replay(AuxiliaryReceipt::Cancelled(receipt))) => {
                transaction.rollback().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error(
                        "roll back cancel run replay",
                        source,
                    ))
                })?;
                Ok(CancelRunOutcome::AlreadyCancelled(receipt))
            }
            Ok(_) => {
                let _ = transaction.rollback().await;
                Err(AuxiliaryTerminalError::InvalidError {
                    reason: "cancel execution returned interruption receipt",
                })
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error(
                        "roll back cancel run",
                        source,
                    ))
                })?;
                Err(error)
            }
        }
    }

    /// Atomically settles a running bound run as interrupted. The run keeps
    /// a NULL `terminal_at_ms`, while its dispatch/item/turn are moved to
    /// their distinct interrupted representations in one commit.
    ///
    /// # Errors
    ///
    /// Returns [`InterruptRunError`] if validation fails, a required database
    /// operation fails, the fenced interruption cannot be applied, or the
    /// transaction produces an invalid auxiliary outcome.
    pub async fn interrupt_run(
        &self,
        command: InterruptRun<'_>,
    ) -> Result<InterruptRunOutcome, InterruptRunError> {
        let auxiliary = AuxiliaryTerminal::Interrupt(&command);
        validate_auxiliary(&auxiliary)?;
        let transaction = self.begin_write().await.map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error("begin interrupt run", source))
        })?;
        match execute_auxiliary(&transaction, &auxiliary).await {
            Ok(AuxiliaryExecution::Persisted(AuxiliaryReceipt::Interrupted(receipt))) => {
                transaction.commit().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error(
                        "commit interrupt run",
                        source,
                    ))
                })?;
                Ok(InterruptRunOutcome::Interrupted(receipt))
            }
            Ok(AuxiliaryExecution::Replay(AuxiliaryReceipt::Interrupted(receipt))) => {
                transaction.rollback().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error(
                        "roll back interrupt run replay",
                        source,
                    ))
                })?;
                Ok(InterruptRunOutcome::AlreadyInterrupted(receipt))
            }
            Ok(_) => {
                let _ = transaction.rollback().await;
                Err(AuxiliaryTerminalError::InvalidError {
                    reason: "interrupt execution returned cancellation receipt",
                })
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    AuxiliaryTerminalError::Repository(database_error(
                        "roll back interrupt run",
                        source,
                    ))
                })?;
                Err(error)
            }
        }
    }
}

impl<'a> AuxiliaryTerminal<'a> {
    fn scope(&self) -> RunBatchScope<'a> {
        match self {
            Self::Cancel(command) => RunBatchScope {
                claimed: command.scope.claimed,
                launched: command.scope.launched,
                bound: command.scope.bound,
                run_start_key: command.scope.run_start_key,
                credentials: command.scope.credentials,
                expected_launch_at: command.scope.expected_launch_at,
                expected_updated_at: command.scope.expected_updated_at,
            },
            Self::Interrupt(command) => RunBatchScope {
                claimed: command.scope.claimed,
                launched: command.scope.launched,
                bound: command.scope.bound,
                run_start_key: command.scope.run_start_key,
                credentials: command.scope.credentials,
                expected_launch_at: command.scope.expected_launch_at,
                expected_updated_at: command.scope.expected_updated_at,
            },
        }
    }

    fn operated_at(&self) -> UnixMillis {
        match self {
            Self::Cancel(command) => command.operated_at,
            Self::Interrupt(command) => command.operated_at,
        }
    }

    fn item_id(&self) -> &'a ItemId {
        match self {
            Self::Cancel(command) => command.item_id,
            Self::Interrupt(command) => command.item_id,
        }
    }

    fn expected_revision(&self) -> Revision {
        match self {
            Self::Cancel(command) => command.expected_revision,
            Self::Interrupt(command) => command.expected_revision,
        }
    }

    fn body(&self) -> &'a AssistantBody {
        match self {
            Self::Cancel(command) => command.body,
            Self::Interrupt(command) => command.body,
        }
    }

    fn phase(&self) -> AssistantMessagePhase {
        match self {
            Self::Cancel(command) => command.phase,
            Self::Interrupt(command) => command.phase,
        }
    }

    fn item_patch_id(&self) -> &'a PatchId {
        match self {
            Self::Cancel(command) => command.item_patch_id,
            Self::Interrupt(command) => command.item_patch_id,
        }
    }

    fn turn_patch_id(&self) -> &'a PatchId {
        match self {
            Self::Cancel(command) => command.turn_patch_id,
            Self::Interrupt(command) => command.turn_patch_id,
        }
    }

    fn run_lifecycle(&self) -> AssistantRunLifecycle {
        match self {
            Self::Cancel(_) => AssistantRunLifecycle::Cancelled,
            Self::Interrupt(_) => AssistantRunLifecycle::Interrupted,
        }
    }

    fn run_lifecycle_label(&self) -> &'static str {
        match self {
            Self::Cancel(_) => "cancelled",
            Self::Interrupt(_) => "interrupted",
        }
    }

    fn entity_lifecycle(&self) -> EntityLifecycle {
        match self {
            Self::Cancel(_) => EntityLifecycle::Cancelled,
            Self::Interrupt(_) => EntityLifecycle::Interrupted,
        }
    }

    fn dispatch_error(&self) -> &'a str {
        match self {
            Self::Cancel(_) => AUX_CANCEL_DISPATCH_ERROR,
            Self::Interrupt(command) => command.error_message.as_str(),
        }
    }

    fn error_pair(&self) -> Option<(&'a RunErrorCode, &'a RunErrorMessage)> {
        match self {
            Self::Cancel(_) => None,
            Self::Interrupt(command) => Some((command.error_code, command.error_message)),
        }
    }

    fn terminal_at(&self) -> Option<UnixMillis> {
        match self {
            Self::Cancel(command) => Some(command.operated_at),
            Self::Interrupt(_) => None,
        }
    }
}

fn validate_auxiliary(command: &AuxiliaryTerminal<'_>) -> Result<(), AuxiliaryTerminalError> {
    let scope = command.scope();
    if scope.launched.generation <= 0
        || scope.launched.generation != scope.bound.generation
        || scope.bound.binding_version <= 0
    {
        return Err(AuxiliaryTerminalError::CredentialMismatch {
            run_id: scope.launched.run_id.clone(),
        });
    }
    if scope.claimed.attempt_count == 0 || i32::try_from(scope.claimed.attempt_count).is_err() {
        return Err(AuxiliaryTerminalError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.claimed.message_id != scope.launched.message_id
        || scope.claimed.message_id != scope.bound.message_id
    {
        return Err(AuxiliaryTerminalError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.launched.run_id != scope.bound.run_id
        || scope.launched.thread_id != scope.bound.thread_id
    {
        return Err(AuxiliaryTerminalError::IdentityConflict {
            reason: "launched and bound identities differ",
        });
    }
    if command.item_patch_id().as_str() == command.turn_patch_id().as_str() {
        return Err(AuxiliaryTerminalError::PatchConflict {
            reason: "item and turn patch identities collide",
        });
    }
    if command.body().as_str().len() > AssistantBody::MAX_BYTES {
        return Err(AuxiliaryTerminalError::BodyTooLong {
            length: command.body().as_str().len(),
            maximum: AssistantBody::MAX_BYTES,
        });
    }
    if i64::try_from(command.expected_revision().get()).is_err() {
        return Err(AuxiliaryTerminalError::CounterOverflow {
            counter: "revision",
            value: i64::MAX,
        });
    }
    validate_terminal_chronology(&scope, command.operated_at())
        .map_err(AuxiliaryTerminalError::Repository)
}

async fn execute_auxiliary(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<AuxiliaryExecution, AuxiliaryTerminalError> {
    if !fence_auxiliary_dispatch(transaction, command).await? {
        if let Some(receipt) = classify_auxiliary_replay(transaction, command).await? {
            return Ok(AuxiliaryExecution::Replay(receipt));
        }
        return Err(classify_auxiliary_dispatch_failure(transaction, command).await);
    }
    if !fence_auxiliary_run(transaction, command).await? {
        if let Some(receipt) = classify_auxiliary_replay(transaction, command).await? {
            return Ok(AuxiliaryExecution::Replay(receipt));
        }
        return Err(classify_auxiliary_run_failure(transaction, command).await);
    }
    let context = load_auxiliary_context(transaction, command).await?;
    persist_auxiliary(transaction, command, context).await?;
    let receipt = match command {
        AuxiliaryTerminal::Cancel(_) => AuxiliaryReceipt::Cancelled(TerminalRunReceipt {
            run_id: command.scope().launched.run_id.clone(),
            generation: command.scope().launched.generation,
            terminal_at: command.operated_at(),
        }),
        AuxiliaryTerminal::Interrupt(_) => AuxiliaryReceipt::Interrupted(InterruptedRunReceipt {
            run_id: command.scope().launched.run_id.clone(),
            generation: command.scope().launched.generation,
            interrupted_at: command.operated_at(),
        }),
    };
    Ok(AuxiliaryExecution::Persisted(receipt))
}

async fn fence_auxiliary_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<bool, AuxiliaryTerminalError> {
    let scope = command.scope();
    let claimed = scope.claimed;
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        AUX_DISPATCH_SQL,
        [
            command.dispatch_error().to_owned().into(),
            millis(command.operated_at()).into(),
            claimed.message_id.as_str().into(),
            claimed.correlation_id.as_str().into(),
            i64::from(claimed.attempt_count).into(),
            millis(claimed.queued_at).into(),
            millis(claimed.available_at).into(),
            claimed.owner.to_storage().into(),
            millis(claimed.lease_expires_at).into(),
            millis(scope.expected_updated_at).into(),
            millis(command.operated_at()).into(),
        ],
    );
    let row = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "fence auxiliary terminal dispatch",
                source,
            ))
        })?;
    Ok(row.is_some())
}

async fn fence_auxiliary_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<bool, AuxiliaryTerminalError> {
    let scope = command.scope();
    let launched = scope.launched;
    let (owner, lease, _) = scope.credentials.parts();
    let (error_code, error_message) =
        command
            .error_pair()
            .map_or((None, None), |(code, message)| {
                (
                    Some(code.as_str().to_owned()),
                    Some(message.as_str().to_owned()),
                )
            });
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        AUX_RUN_SQL,
        [
            command.run_lifecycle_label().into(),
            error_code.into(),
            error_message.into(),
            command.terminal_at().map(millis).into(),
            millis(command.operated_at()).into(),
            launched.run_id.as_str().into(),
            launched.thread_id.as_str().into(),
            launched.message_id.as_str().into(),
            launched.turn_id.as_str().into(),
            launched.generation.into(),
            scope.run_start_key.expose().to_vec().into(),
            owner.expose().to_vec().into(),
            lease.expose().to_vec().into(),
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
            AuxiliaryTerminalError::Repository(database_error(
                "fence auxiliary terminal run",
                source,
            ))
        })?;
    Ok(row.is_some())
}

async fn classify_auxiliary_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<Option<AuxiliaryReceipt>, AuxiliaryTerminalError> {
    let scope = command.scope();
    let Some(dispatch) =
        load_auxiliary_replay_dispatch(transaction, scope.claimed.message_id.as_str()).await?
    else {
        return Ok(None);
    };
    if !auxiliary_dispatch_replay_matches(&dispatch, &scope, command) {
        return Ok(None);
    }
    let Some(run) = load_auxiliary_replay_run(transaction, scope.launched.run_id.as_str()).await?
    else {
        return Ok(None);
    };
    if !auxiliary_run_replay_matches(&run, &scope, command) {
        return Ok(None);
    }
    let Some(item) = load_auxiliary_replay_item(transaction, command.item_id().as_str()).await?
    else {
        return Ok(None);
    };
    if !auxiliary_item_replay_matches(&item, &scope, command) {
        return Ok(None);
    }
    let Some(turn) =
        load_auxiliary_replay_turn(transaction, scope.launched.turn_id.as_str()).await?
    else {
        return Ok(None);
    };
    if !auxiliary_turn_replay_matches(&turn, &scope, command) {
        return Ok(None);
    }
    let Some(item_patch) = load_auxiliary_replay_patch(
        transaction,
        command.item_patch_id().as_str(),
        "classify auxiliary item patch replay",
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(turn_patch) = load_auxiliary_replay_patch(
        transaction,
        command.turn_patch_id().as_str(),
        "classify auxiliary turn patch replay",
    )
    .await?
    else {
        return Ok(None);
    };
    if !auxiliary_patches_replay_match(&item_patch, &turn_patch, &scope, command) {
        return Ok(None);
    }
    Ok(Some(auxiliary_replay_receipt(command, &scope)))
}

async fn load_auxiliary_replay_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    message_id: &str,
) -> Result<Option<entities::MessageDispatch>, AuxiliaryTerminalError> {
    entities::message_dispatch::Entity::find_by_id(message_id)
        .one(transaction)
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary dispatch replay",
                source,
            ))
        })
}

async fn load_auxiliary_replay_run(
    transaction: &sea_orm::DatabaseTransaction,
    run_id: &str,
) -> Result<Option<entities::AssistantRun>, AuxiliaryTerminalError> {
    entities::assistant_run::Entity::find_by_id(run_id)
        .one(transaction)
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary run replay",
                source,
            ))
        })
}

async fn load_auxiliary_replay_item(
    transaction: &sea_orm::DatabaseTransaction,
    item_id: &str,
) -> Result<Option<entities::ConversationItem>, AuxiliaryTerminalError> {
    entities::conversation_item::Entity::find_by_id(item_id)
        .one(transaction)
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary item replay",
                source,
            ))
        })
}

async fn load_auxiliary_replay_turn(
    transaction: &sea_orm::DatabaseTransaction,
    turn_id: &str,
) -> Result<Option<entities::ConversationTurn>, AuxiliaryTerminalError> {
    entities::conversation_turn::Entity::find_by_id(turn_id)
        .one(transaction)
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary turn replay",
                source,
            ))
        })
}

async fn load_auxiliary_replay_patch(
    transaction: &sea_orm::DatabaseTransaction,
    patch_id: &str,
    operation: &'static str,
) -> Result<Option<entities::ConversationPatch>, AuxiliaryTerminalError> {
    entities::conversation_patch::Entity::find_by_id(patch_id)
        .one(transaction)
        .await
        .map_err(|source| AuxiliaryTerminalError::Repository(database_error(operation, source)))
}

fn auxiliary_dispatch_replay_matches(
    dispatch: &entities::MessageDispatch,
    scope: &RunBatchScope<'_>,
    command: &AuxiliaryTerminal<'_>,
) -> bool {
    dispatch.state == DispatchState::Failed
        && dispatch.correlation_id == scope.claimed.correlation_id.as_str()
        && dispatch.attempt_count == i32::try_from(scope.claimed.attempt_count).unwrap_or(-1)
        && dispatch.queued_at_ms == millis(scope.claimed.queued_at)
        && dispatch.available_at_ms == millis(scope.claimed.available_at)
        && dispatch.updated_at_ms == millis(command.operated_at())
        && dispatch.lease_owner.is_none()
        && dispatch.lease_expires_at_ms.is_none()
        && dispatch.last_error.as_deref() == Some(command.dispatch_error())
}

fn auxiliary_run_replay_matches(
    run: &entities::AssistantRun,
    scope: &RunBatchScope<'_>,
    command: &AuxiliaryTerminal<'_>,
) -> bool {
    if run.lifecycle != command.run_lifecycle()
        || run.updated_at_ms != millis(command.operated_at())
        || run.thread_id != scope.launched.thread_id.as_str()
        || run.origin_message_id != scope.launched.message_id.as_str()
        || run.origin_turn_id != scope.launched.turn_id.as_str()
        || run.generation != scope.launched.generation
        || !stored_bytes_match(&run.run_start_key, scope.run_start_key.expose())
        || run.owner.is_some()
        || run.lease.is_some()
        || run.claim_token.is_some()
        || run.provider_binding_version != Some(scope.bound.binding_version)
        || run.provider_binding.is_none()
        || run.provider_bound_at_ms != Some(millis(scope.bound.bound_at))
    {
        return false;
    }
    match command.error_pair() {
        Some((code, message)) => {
            run.terminal_at_ms.is_none()
                && run.error_code.as_deref() == Some(code.as_str())
                && run.error_message.as_deref() == Some(message.as_str())
        }
        None => {
            run.terminal_at_ms == command.terminal_at().map(millis)
                && run.error_code.is_none()
                && run.error_message.is_none()
        }
    }
}

fn auxiliary_item_replay_matches(
    item: &entities::ConversationItem,
    scope: &RunBatchScope<'_>,
    command: &AuxiliaryTerminal<'_>,
) -> bool {
    let expected_revision = i64::try_from(command.expected_revision().get())
        .ok()
        .and_then(|revision| revision.checked_add(1));
    item.lifecycle == command.entity_lifecycle()
        && item.thread_id == scope.launched.thread_id.as_str()
        && item.turn_id == scope.launched.turn_id.as_str()
        && item.run_id.as_deref() == Some(scope.launched.run_id.as_str())
        && item.item_kind == ConversationItemKind::AssistantMessage
        && item.body == command.body().as_str()
        && item.phase.as_ref() == Some(&map_phase(command.phase()))
        && item.revision == expected_revision.unwrap_or(-1)
        && item.updated_at_ms == millis(command.operated_at())
}

fn auxiliary_turn_replay_matches(
    turn: &entities::ConversationTurn,
    scope: &RunBatchScope<'_>,
    command: &AuxiliaryTerminal<'_>,
) -> bool {
    turn.lifecycle == command.entity_lifecycle()
        && turn.thread_id == scope.launched.thread_id.as_str()
        && turn.updated_at_ms == millis(command.operated_at())
}

fn auxiliary_patches_replay_match(
    item_patch: &entities::ConversationPatch,
    turn_patch: &entities::ConversationPatch,
    scope: &RunBatchScope<'_>,
    command: &AuxiliaryTerminal<'_>,
) -> bool {
    item_patch.thread_id == scope.launched.thread_id.as_str()
        && item_patch.kind == ConversationPatchKind::ItemLifecycle
        && item_patch.item_id.as_deref() == Some(command.item_id().as_str())
        && item_patch.lifecycle == Some(command.entity_lifecycle())
        && turn_patch.thread_id == scope.launched.thread_id.as_str()
        && turn_patch.kind == ConversationPatchKind::TurnLifecycle
        && turn_patch.turn_id.as_deref() == Some(scope.launched.turn_id.as_str())
        && turn_patch.lifecycle == Some(command.entity_lifecycle())
}

fn auxiliary_replay_receipt(
    command: &AuxiliaryTerminal<'_>,
    scope: &RunBatchScope<'_>,
) -> AuxiliaryReceipt {
    match command {
        AuxiliaryTerminal::Cancel(_) => AuxiliaryReceipt::Cancelled(TerminalRunReceipt {
            run_id: scope.launched.run_id.clone(),
            generation: scope.launched.generation,
            terminal_at: command.operated_at(),
        }),
        AuxiliaryTerminal::Interrupt(_) => AuxiliaryReceipt::Interrupted(InterruptedRunReceipt {
            run_id: scope.launched.run_id.clone(),
            generation: scope.launched.generation,
            interrupted_at: command.operated_at(),
        }),
    }
}

async fn classify_auxiliary_dispatch_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> AuxiliaryTerminalError {
    let claimed = command.scope().claimed;
    let operated_at_ms = millis(command.operated_at());
    let dispatch = match entities::message_dispatch::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(dispatch)) => dispatch,
        Ok(None) => {
            return AuxiliaryTerminalError::Repository(RepositoryError::DispatchNotFound {
                message_id: claimed.message_id.clone(),
            });
        }
        Err(source) => {
            return AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary dispatch",
                source,
            ));
        }
    };
    if dispatch.state != DispatchState::Running {
        return AuxiliaryTerminalError::Repository(RepositoryError::InvalidDispatchState {
            message_id: claimed.message_id.clone(),
            state: dispatch_state_label(&dispatch.state),
        });
    }
    if let Some(expiry) = dispatch.lease_expires_at_ms
        && expiry <= operated_at_ms
    {
        return AuxiliaryTerminalError::Repository(RepositoryError::DispatchLeaseExpired {
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
        return AuxiliaryTerminalError::Repository(RepositoryError::DispatchOwnerMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    AuxiliaryTerminalError::SnapshotMismatch {
        message_id: claimed.message_id.clone(),
    }
}

async fn classify_auxiliary_run_failure(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> AuxiliaryTerminalError {
    let scope = command.scope();
    let run = match entities::assistant_run::Entity::find_by_id(scope.launched.run_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(run)) => run,
        Ok(None) => {
            return AuxiliaryTerminalError::RunNotFound {
                run_id: scope.launched.run_id.clone(),
            };
        }
        Err(source) => {
            return AuxiliaryTerminalError::Repository(database_error(
                "classify auxiliary run",
                source,
            ));
        }
    };
    if run.lifecycle != AssistantRunLifecycle::Running {
        return AuxiliaryTerminalError::RunNotRunning {
            run_id: scope.launched.run_id.clone(),
        };
    }
    if run.thread_id != scope.launched.thread_id.as_str()
        || run.origin_message_id != scope.launched.message_id.as_str()
        || run.origin_turn_id != scope.launched.turn_id.as_str()
    {
        return AuxiliaryTerminalError::IdentityConflict {
            reason: "stored run originates from another thread, message, or turn",
        };
    }
    let (owner, lease, _) = scope.credentials.parts();
    if run.generation != scope.launched.generation
        || !stored_bytes_match(&run.run_start_key, scope.run_start_key.expose())
        || !owner.matches_stored(run.owner.as_ref())
        || !lease.matches_stored(run.lease.as_ref())
        || run.claim_token.is_some()
        || run.provider_binding_version != Some(scope.bound.binding_version)
        || run.provider_binding.is_none()
        || run.provider_bound_at_ms != Some(millis(scope.bound.bound_at))
    {
        return AuxiliaryTerminalError::CredentialMismatch {
            run_id: scope.launched.run_id.clone(),
        };
    }
    AuxiliaryTerminalError::SnapshotMismatch {
        message_id: scope.claimed.message_id.clone(),
    }
}

async fn load_auxiliary_context(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<AuxiliaryContext, AuxiliaryTerminalError> {
    let scope = command.scope();
    let thread_id = scope.launched.thread_id.as_str();
    let state = load_auxiliary_state(transaction, command, thread_id).await?;

    let turn = load_auxiliary_turn(
        transaction,
        command,
        thread_id,
        scope.launched.turn_id.as_str(),
    )
    .await?;

    let item = load_auxiliary_item(
        transaction,
        command,
        thread_id,
        scope.launched.turn_id.as_str(),
        scope.launched.run_id.as_str(),
    )
    .await?;
    ensure_auxiliary_patches_vacant(transaction, command).await?;
    Ok(AuxiliaryContext { state, turn, item })
}

async fn load_auxiliary_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    thread_id: &str,
) -> Result<projection::LoadedState, AuxiliaryTerminalError> {
    let state = projection::load_conversation_state(transaction, thread_id)
        .await
        .map_err(map_projection_error_auxiliary)?
        .ok_or_else(|| {
            AuxiliaryTerminalError::Repository(corrupt_data(
                "conversation_state",
                "thread_id",
                "fenced auxiliary terminal found no conversation state",
            ))
        })?;
    if state.next_renderer_ordinal < 0 || state.last_patch_sequence < 0 {
        return Err(AuxiliaryTerminalError::Repository(corrupt_data(
            "conversation_state",
            "counter",
            "counter is negative",
        )));
    }
    if command.operated_at().as_millis() < state.updated_at_ms {
        return Err(AuxiliaryTerminalError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_state.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if state.last_patch_sequence.checked_add(2).is_none() {
        return Err(AuxiliaryTerminalError::CounterOverflow {
            counter: "patch sequence",
            value: state.last_patch_sequence,
        });
    }
    Ok(state)
}

async fn load_auxiliary_turn(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    thread_id: &str,
    turn_id: &str,
) -> Result<projection::LoadedTurn, AuxiliaryTerminalError> {
    let turn = projection::load_turn(transaction, turn_id)
        .await
        .map_err(map_projection_error_auxiliary)?
        .ok_or_else(|| {
            AuxiliaryTerminalError::Repository(corrupt_data(
                "conversation_turns",
                "turn_id",
                "fenced auxiliary terminal lost its origin turn",
            ))
        })?;
    if turn.thread_id != thread_id {
        return Err(AuxiliaryTerminalError::Repository(corrupt_data(
            "conversation_turns",
            "thread_id",
            "origin turn belongs to another thread",
        )));
    }
    if command.operated_at().as_millis() < turn.updated_at_ms {
        return Err(AuxiliaryTerminalError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_turns.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    if matches!(
        turn.lifecycle,
        EntityLifecycle::Completed
            | EntityLifecycle::Failed
            | EntityLifecycle::Interrupted
            | EntityLifecycle::Cancelled
    ) {
        return Err(AuxiliaryTerminalError::TargetConflict {
            reason: "origin turn is sealed",
        });
    }
    Ok(turn)
}

async fn load_auxiliary_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    thread_id: &str,
    turn_id: &str,
    run_id: &str,
) -> Result<entities::ConversationItem, AuxiliaryTerminalError> {
    let item = projection::load_item(transaction, command.item_id().as_str())
        .await
        .map_err(map_projection_error_auxiliary)?
        .ok_or(AuxiliaryTerminalError::TargetConflict {
            reason: "target item does not exist",
        })?;
    if item.thread_id != thread_id
        || item.turn_id != turn_id
        || item.run_id.as_deref() != Some(run_id)
        || item.item_kind != ConversationItemKind::AssistantMessage
    {
        return Err(AuxiliaryTerminalError::TargetConflict {
            reason: "target item belongs to another run, turn, or thread",
        });
    }
    if matches!(
        item.lifecycle,
        EntityLifecycle::Completed
            | EntityLifecycle::Failed
            | EntityLifecycle::Interrupted
            | EntityLifecycle::Cancelled
    ) {
        return Err(AuxiliaryTerminalError::SealedItem {
            item_id: command.item_id().clone(),
        });
    }
    if command.operated_at().as_millis() < item.updated_at_ms {
        return Err(AuxiliaryTerminalError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "conversation_items.updated_at_ms",
                later_field: "terminal operated_at",
            },
        ));
    }
    let stored_revision = u64::try_from(item.revision).map_err(|_| {
        AuxiliaryTerminalError::Repository(corrupt_data(
            "conversation_items",
            "revision",
            "revision is negative",
        ))
    })?;
    if stored_revision != command.expected_revision().get() {
        return Err(AuxiliaryTerminalError::TargetConflict {
            reason: "expected revision does not match the stored item",
        });
    }
    Ok(item)
}

async fn ensure_auxiliary_patches_vacant(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
) -> Result<(), AuxiliaryTerminalError> {
    if projection::patch_exists(transaction, command.item_patch_id().as_str())
        .await
        .map_err(map_projection_error_auxiliary)?
        || projection::patch_exists(transaction, command.turn_patch_id().as_str())
            .await
            .map_err(map_projection_error_auxiliary)?
    {
        return Err(AuxiliaryTerminalError::PatchConflict {
            reason: "terminal patch identity already exists",
        });
    }
    Ok(())
}

fn map_projection_error_auxiliary(
    error: super::super::RunObservationError,
) -> AuxiliaryTerminalError {
    match error {
        super::super::RunObservationError::Repository(inner) => {
            AuxiliaryTerminalError::Repository(inner)
        }
        other => AuxiliaryTerminalError::Repository(RepositoryError::Database {
            operation: "load auxiliary terminal context",
            source: sea_orm::DbErr::Custom(format!("{other:?}")),
        }),
    }
}

struct AuxiliaryPersistencePlan {
    item_revision: i64,
    turn_revision: i64,
    first_sequence: i64,
    second_sequence: i64,
    operated_at_ms: i64,
    lifecycle: EntityLifecycle,
    lifecycle_label: &'static str,
    phase: &'static str,
}

async fn persist_auxiliary(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    context: AuxiliaryContext,
) -> Result<(), AuxiliaryTerminalError> {
    let plan = auxiliary_persistence_plan(command, &context)?;
    update_auxiliary_item(transaction, command, &context, &plan).await?;
    update_auxiliary_turn(transaction, &context, &plan).await?;
    insert_auxiliary_patches(transaction, command, &context, &plan).await?;
    advance_auxiliary_state(transaction, command, &context, &plan).await?;
    Ok(())
}

fn auxiliary_persistence_plan(
    command: &AuxiliaryTerminal<'_>,
    context: &AuxiliaryContext,
) -> Result<AuxiliaryPersistencePlan, AuxiliaryTerminalError> {
    let item_revision = next_revision_auxiliary("conversation_items", context.item.revision)?;
    let turn_revision = next_revision_auxiliary("conversation_turns", context.turn.revision)?;
    let first_sequence = context.state.last_patch_sequence.checked_add(1).ok_or(
        AuxiliaryTerminalError::CounterOverflow {
            counter: "patch sequence",
            value: context.state.last_patch_sequence,
        },
    )?;
    let second_sequence =
        first_sequence
            .checked_add(1)
            .ok_or(AuxiliaryTerminalError::CounterOverflow {
                counter: "patch sequence",
                value: first_sequence,
            })?;
    let operated_at_ms = millis(command.operated_at());
    let lifecycle = command.entity_lifecycle();
    let lifecycle_label = match &lifecycle {
        EntityLifecycle::Cancelled => "cancelled",
        EntityLifecycle::Interrupted => "interrupted",
        _ => unreachable!("auxiliary terminal lifecycle is always terminal"),
    };
    let phase = render_phase_label(&map_phase(command.phase()));
    Ok(AuxiliaryPersistencePlan {
        item_revision,
        turn_revision,
        first_sequence,
        second_sequence,
        operated_at_ms,
        lifecycle,
        lifecycle_label,
        phase,
    })
}

async fn update_auxiliary_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    context: &AuxiliaryContext,
    plan: &AuxiliaryPersistencePlan,
) -> Result<(), AuxiliaryTerminalError> {
    let item_updated = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r"
UPDATE conversation_items
SET lifecycle = ?, body = ?, phase = ?, revision = ?, updated_at_ms = ?
WHERE item_id = ? AND revision = ?
RETURNING item_id
",
            [
                plan.lifecycle_label.into(),
                command.body().as_str().to_owned().into(),
                plan.phase.into(),
                plan.item_revision.into(),
                plan.operated_at_ms.into(),
                command.item_id().as_str().into(),
                context.item.revision.into(),
            ],
        ))
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "update auxiliary terminal item",
                source,
            ))
        })?;
    if item_updated.is_none() {
        return Err(AuxiliaryTerminalError::TargetConflict {
            reason: "item fence failed",
        });
    }
    Ok(())
}

async fn update_auxiliary_turn(
    transaction: &sea_orm::DatabaseTransaction,
    context: &AuxiliaryContext,
    plan: &AuxiliaryPersistencePlan,
) -> Result<(), AuxiliaryTerminalError> {
    let turn_updated = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r"
UPDATE conversation_turns
SET lifecycle = ?, revision = ?, updated_at_ms = ?
WHERE turn_id = ? AND revision = ?
RETURNING turn_id
",
            [
                plan.lifecycle_label.into(),
                plan.turn_revision.into(),
                plan.operated_at_ms.into(),
                context.turn.turn_id.clone().into(),
                context.turn.revision.into(),
            ],
        ))
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "update auxiliary terminal turn",
                source,
            ))
        })?;
    if turn_updated.is_none() {
        return Err(AuxiliaryTerminalError::TargetConflict {
            reason: "turn fence failed",
        });
    }
    Ok(())
}

async fn insert_auxiliary_patches(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    context: &AuxiliaryContext,
    plan: &AuxiliaryPersistencePlan,
) -> Result<(), AuxiliaryTerminalError> {
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope().launched.thread_id.as_str(),
            patch_id: command.item_patch_id().as_str(),
            sequence: plan.first_sequence,
            kind: ConversationPatchKind::ItemLifecycle,
            revision: plan.item_revision,
            recorded_at_ms: plan.operated_at_ms,
            item_id: Some(command.item_id().as_str()),
            turn_id: None,
            lifecycle: Some(plan.lifecycle.clone()),
        },
    )
    .await
    .map_err(AuxiliaryTerminalError::Repository)?;
    insert_terminal_patch(
        transaction,
        TerminalPatchInput {
            thread_id: command.scope().launched.thread_id.as_str(),
            patch_id: command.turn_patch_id().as_str(),
            sequence: plan.second_sequence,
            kind: ConversationPatchKind::TurnLifecycle,
            revision: plan.turn_revision,
            recorded_at_ms: plan.operated_at_ms,
            item_id: None,
            turn_id: Some(context.turn.turn_id.as_str()),
            lifecycle: Some(plan.lifecycle.clone()),
        },
    )
    .await
    .map_err(AuxiliaryTerminalError::Repository)?;
    Ok(())
}

async fn advance_auxiliary_state(
    transaction: &sea_orm::DatabaseTransaction,
    command: &AuxiliaryTerminal<'_>,
    context: &AuxiliaryContext,
    plan: &AuxiliaryPersistencePlan,
) -> Result<(), AuxiliaryTerminalError> {
    let advanced = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r"
UPDATE conversation_state
SET last_patch_sequence = ?, updated_at_ms = ?
WHERE thread_id = ? AND last_patch_sequence = ?
RETURNING thread_id
",
            [
                plan.second_sequence.into(),
                plan.operated_at_ms.into(),
                command.scope().launched.thread_id.as_str().into(),
                context.state.last_patch_sequence.into(),
            ],
        ))
        .await
        .map_err(|source| {
            AuxiliaryTerminalError::Repository(database_error(
                "advance auxiliary terminal state",
                source,
            ))
        })?;
    if advanced.is_none() {
        return Err(AuxiliaryTerminalError::SnapshotMismatch {
            message_id: command.scope().claimed.message_id.clone(),
        });
    }
    Ok(())
}

fn next_revision_auxiliary(
    table: &'static str,
    current: i64,
) -> Result<i64, AuxiliaryTerminalError> {
    let current = u64::try_from(current).map_err(|_| {
        AuxiliaryTerminalError::Repository(corrupt_data(table, "revision", "revision is negative"))
    })?;
    let next = Revision::new(current).checked_next().map_err(|_| {
        AuxiliaryTerminalError::CounterOverflow {
            counter: "revision",
            value: i64::try_from(current).unwrap_or(i64::MAX),
        }
    })?;
    i64::try_from(next.get()).map_err(|_| AuxiliaryTerminalError::CounterOverflow {
        counter: "revision",
        value: i64::MAX,
    })
}
