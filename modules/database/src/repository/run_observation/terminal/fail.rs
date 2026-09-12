//! Fail-run settlement: error-carrying terminal co-commit and replay.

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

/// Typed outcome of `fail_run`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailRunOutcome {
    /// This transaction failed the run.
    Failed(TerminalRunReceipt),
    /// An earlier identical transaction already failed this run.
    AlreadyFailed(TerminalRunReceipt),
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

impl Repository {
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
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| FailRunError::Repository(database_error("begin fail run", source)))?;
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

enum FailExecution {
    Persisted(TerminalRunReceipt),
    Replay(TerminalRunReceipt),
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

fn map_projection_error_fail(error: super::super::RunObservationError) -> FailRunError {
    match error {
        super::super::RunObservationError::Repository(inner) => FailRunError::Repository(inner),
        other => FailRunError::Repository(RepositoryError::Database {
            operation: "load terminal context",
            source: sea_orm::DbErr::Custom(format!("{other:?}")),
        }),
    }
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
