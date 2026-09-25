//! Complete-run settlement: dispatch, run, item, turn, and patch co-commit.

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
    TerminalPatchInput, TerminalRunReceipt, dispatch_state_label, insert_terminal_patch, map_phase,
    render_phase_label, validate_terminal_chronology,
};

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

/// Typed outcome of `complete_run`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompleteRunOutcome {
    /// This transaction completed the run.
    Completed(TerminalRunReceipt),
    /// An earlier identical transaction already completed this run.
    AlreadyCompleted(TerminalRunReceipt),
}

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
  AND lease_expires_at_ms >= ?
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
        let transaction = self.begin_write().await.map_err(|source| {
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
}

enum CompleteExecution {
    Persisted(TerminalRunReceipt),
    Replay(TerminalRunReceipt),
}

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

fn map_projection_error_complete(error: super::super::RunObservationError) -> CompleteRunError {
    match error {
        super::super::RunObservationError::Repository(inner) => CompleteRunError::Repository(inner),
        other => CompleteRunError::Repository(RepositoryError::Database {
            operation: "load terminal context",
            source: sea_orm::DbErr::Custom(format!("{other:?}")),
        }),
    }
}

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
