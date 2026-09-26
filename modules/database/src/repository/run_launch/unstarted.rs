//! Atomic failure of one launched run whose provider never started.
//!
//! The dispatcher owns a launched run from `launch_claimed_run` until the
//! provider announces its session and the run binds. When the provider fails
//! to start (or misses the dispatcher's launch deadline) the dispatcher has
//! already torn the child down, so it knows the outcome: the run failed
//! before any provider session existed. This transaction records exactly
//! that, while the dispatcher still holds the live dispatch lease, instead of
//! leaving the pair for lease-expiry recovery to report an unknown outcome.
//!
//! One transaction fences the live dispatch (owner, `running`, unexpired
//! lease) and the unbound `launching` run (identity, start key, all three
//! launch capabilities, no binding), then fails the dispatch with the given
//! reason, fails the run with the given error, fails the origin turn, and
//! appends its `turn_lifecycle` patch with the per-thread counter. Any fence
//! miss rolls back and answers [`FailUnstartedRunOutcome::Moved`]: someone
//! else (a bind, a cancel, or recovery) already owns the pair.

use artisan_domain::{PatchId, UnixMillis};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DbBackend, EntityTrait, Statement,
};

use crate::entities::{self, ConversationPatchKind, EntityLifecycle};
use crate::repository::message_dispatch::{ClaimedMessageDispatch, DispatchFailureReason};
use crate::repository::run_observation::terminal::{RunErrorCode, RunErrorMessage};
use crate::repository::{Repository, RepositoryError, corrupt_data, database_error, millis};

use super::{LaunchedRunReceipt, RunLaunchCredentials, RunStartKey};

const FAIL_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'running'
  AND lease_owner = ?
  AND lease_expires_at_ms > ?
  AND updated_at_ms <= ?
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
  AND generation = ?
  AND lifecycle = 'launching'
  AND run_start_key = ?
  AND owner = ?
  AND lease = ?
  AND claim_token = ?
  AND provider_binding IS NULL
  AND provider_binding_version IS NULL
  AND provider_bound_at_ms IS NULL
  AND terminal_at_ms IS NULL
  AND updated_at_ms <= ?
RETURNING run_id
";

/// Borrowed inputs of one unstarted-run failure.
pub struct FailUnstartedRun<'a> {
    /// The live claim; its owner token is the dispatch authority.
    pub claimed: &'a ClaimedMessageDispatch,
    /// Receipt of the launch that created the run.
    pub receipt: &'a LaunchedRunReceipt,
    /// Start key the run was launched with.
    pub run_start_key: &'a RunStartKey,
    /// Launch capabilities the run was launched with.
    pub credentials: &'a RunLaunchCredentials,
    /// Caller-injected failure time; must precede the lease expiry.
    pub operated_at: UnixMillis,
    /// Caller-minted `turn_lifecycle` patch identity.
    pub turn_patch_id: &'a PatchId,
    /// Bounded run error code.
    pub error_code: &'a RunErrorCode,
    /// Bounded run error message.
    pub error_message: &'a RunErrorMessage,
    /// Failure reason the composer shows for the message.
    pub dispatch_reason: &'a DispatchFailureReason,
}

/// Typed outcome of [`Repository::fail_unstarted_run`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailUnstartedRunOutcome {
    /// This transaction failed the dispatch, run, and turn.
    Failed,
    /// The pair no longer matched the live launch; nothing changed.
    Moved,
}

impl Repository {
    /// Fails one launched run whose provider never started.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] for database failures, a corrupt or sealed
    /// origin turn, or an exhausted counter; every error rolls back.
    pub async fn fail_unstarted_run(
        &self,
        command: FailUnstartedRun<'_>,
    ) -> Result<FailUnstartedRunOutcome, RepositoryError> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin unstarted-run failure", source))?;
        match execute(&transaction, &command).await {
            Ok(FailUnstartedRunOutcome::Failed) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit unstarted-run failure", source))?;
                Ok(FailUnstartedRunOutcome::Failed)
            }
            Ok(FailUnstartedRunOutcome::Moved) => {
                let _ = transaction.rollback().await;
                Ok(FailUnstartedRunOutcome::Moved)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}

async fn execute(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailUnstartedRun<'_>,
) -> Result<FailUnstartedRunOutcome, RepositoryError> {
    let operated_at_ms = millis(command.operated_at);
    let receipt = command.receipt;
    let dispatch = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        FAIL_DISPATCH_SQL,
        [
            command.dispatch_reason.as_str().to_owned().into(),
            operated_at_ms.into(),
            command.claimed.message_id.as_str().into(),
            command.claimed.owner.to_storage().into(),
            operated_at_ms.into(),
            operated_at_ms.into(),
        ],
    );
    let fenced = transaction
        .query_one_raw(dispatch)
        .await
        .map_err(|source| database_error("fence unstarted-run dispatch", source))?;
    if fenced.is_none() || receipt.message_id != command.claimed.message_id {
        return Ok(FailUnstartedRunOutcome::Moved);
    }
    let (owner, lease, claim_token) = command.credentials.parts();
    let run = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        FAIL_RUN_SQL,
        [
            command.error_code.as_str().to_owned().into(),
            command.error_message.as_str().to_owned().into(),
            operated_at_ms.into(),
            operated_at_ms.into(),
            receipt.run_id.as_str().into(),
            receipt.thread_id.as_str().into(),
            receipt.message_id.as_str().into(),
            receipt.turn_id.as_str().into(),
            receipt.generation.into(),
            command.run_start_key.expose().to_vec().into(),
            owner.expose().to_vec().into(),
            lease.expose().to_vec().into(),
            claim_token.expose().to_vec().into(),
            operated_at_ms.into(),
        ],
    );
    let fenced = transaction
        .query_one_raw(run)
        .await
        .map_err(|source| database_error("fence unstarted run", source))?;
    if fenced.is_none() {
        return Ok(FailUnstartedRunOutcome::Moved);
    }
    fail_origin_turn(transaction, command, operated_at_ms).await?;
    Ok(FailUnstartedRunOutcome::Failed)
}

async fn fail_origin_turn(
    transaction: &sea_orm::DatabaseTransaction,
    command: &FailUnstartedRun<'_>,
    operated_at_ms: i64,
) -> Result<(), RepositoryError> {
    let thread_id = command.receipt.thread_id.as_str();
    let turn = entities::conversation_turn::Entity::find_by_id(command.receipt.turn_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| database_error("load unstarted-run turn", source))?
        .ok_or_else(|| {
            corrupt_data(
                "conversation_turns",
                "turn_id",
                "launched run lost its origin turn",
            )
        })?;
    if turn.thread_id != thread_id
        || matches!(
            turn.lifecycle,
            EntityLifecycle::Completed
                | EntityLifecycle::Failed
                | EntityLifecycle::Cancelled
                | EntityLifecycle::Interrupted
        )
    {
        return Err(RepositoryError::Invariant {
            reason: "unstarted run's origin turn is foreign or already sealed",
        });
    }
    let state = entities::conversation_state::Entity::find_by_id(thread_id)
        .one(transaction)
        .await
        .map_err(|source| database_error("load unstarted-run conversation state", source))?
        .ok_or_else(|| {
            corrupt_data(
                "conversation_state",
                "thread_id",
                "launched run has no conversation state",
            )
        })?;
    let revision = turn
        .revision
        .checked_add(1)
        .ok_or(RepositoryError::Invariant {
            reason: "turn revision exhausted",
        })?;
    let sequence = state
        .last_patch_sequence
        .checked_add(1)
        .ok_or(RepositoryError::Invariant {
            reason: "patch sequence exhausted",
        })?;
    let turn_updated_at_ms = operated_at_ms.max(turn.updated_at_ms);
    let state_updated_at_ms = operated_at_ms.max(state.updated_at_ms);
    let mut turn: entities::conversation_turn::ActiveModel = turn.into();
    turn.lifecycle = Set(EntityLifecycle::Failed);
    turn.revision = Set(revision);
    turn.updated_at_ms = Set(turn_updated_at_ms);
    turn.update(transaction)
        .await
        .map_err(|source| database_error("fail unstarted-run turn", source))?;
    entities::conversation_patch::ActiveModel {
        patch_id: Set(command.turn_patch_id.as_str().to_owned()),
        thread_id: Set(thread_id.to_owned()),
        sequence: Set(sequence),
        kind: Set(ConversationPatchKind::TurnLifecycle),
        revision: Set(revision),
        recorded_at_ms: Set(operated_at_ms),
        turn_id: Set(Some(command.receipt.turn_id.as_str().to_owned())),
        item_id: Set(None),
        ordinal: Set(None),
        lifecycle: Set(Some(EntityLifecycle::Failed)),
        item_kind: Set(None),
        run_id: Set(None),
        phase: Set(None),
        body: Set(None),
        fragment: Set(None),
        entity_created_at_ms: Set(None),
        entity_updated_at_ms: Set(None),
    }
    .insert(transaction)
    .await
    .map_err(|source| database_error("insert unstarted-run turn patch", source))?;
    let mut state: entities::conversation_state::ActiveModel = state.into();
    state.last_patch_sequence = Set(sequence);
    state.updated_at_ms = Set(state_updated_at_ms);
    state
        .update(transaction)
        .await
        .map_err(|source| database_error("advance unstarted-run patch sequence", source))?;
    Ok(())
}
