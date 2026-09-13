//! Batch commit transaction core and its private helpers.

use artisan_domain::{AssistantBody, ItemId, PatchId, Revision};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement};
use serde_json::Value;

use crate::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, DispatchState, EntityLifecycle,
};
use crate::repository::message_dispatch::DispatchLeaseOwner;
use crate::repository::run_launch::stored_bytes_match;
use crate::repository::{Repository, RepositoryError, corrupt_data, database_error, millis};

use super::codec::{
    DecodedObservationBatch, OBSERVATION_CHECKPOINT_VERSION, OBSERVATION_FORMAT_TAG,
    decode_observation_checkpoint, encode_observation_bytes,
};
use super::{
    AssistantChange, COMMIT_DISPATCH_SQL, COMMIT_RUN_SQL, CheckpointUpdate, CommitRunBatch,
    CommitRunBatchOutcome, RunBatchReceiptInfo, RunObservationError, batch, projection,
};

impl Repository {
    /// Atomically commits one RUNNING progress/checkpoint batch.
    ///
    /// Pure validation and the canonical v1 digest happen before any SQL.
    /// One transaction then fences the RUNNING dispatch stamp from
    /// `expected_updated_at` to `operated_at`, classifies any existing
    /// `(run_id, batch_sequence)` receipt (an exact committed replay answers
    /// [`CommitRunBatchOutcome::AlreadyCommitted`] after rolling back the
    /// tentative stamp; every divergence is a typed conflict), fences the
    /// running run row (advancing only `updated_at_ms`), loads and validates
    /// the checkpoint, conversation state, origin turn, and every target
    /// item through the same transaction, then persists the ordinal ledger,
    /// item, patch, counter, checkpoint, and receipt effects and commits
    /// once. When the checkpoint carries the typed observation tag, every
    /// observation in the batch is also appended to the thread-scoped
    /// observation ledger in the same transaction; `Keep` and opaque
    /// non-observation checkpoints append nothing, and an exact receipt
    /// replay appends nothing. Any failure explicitly rolls back everything
    /// including the tentative fences. A commit error has unknown outcome:
    /// the caller may retry the exact command for receipt classification but
    /// must never reissue an external prompt.
    ///
    /// # Errors
    ///
    /// Returns [`RunObservationError::Repository`] for existing typed
    /// repository rejections (chronology, expired lease, dispatch state and
    /// owner mismatches, corrupt data, database failures), and the typed
    /// variants of [`RunObservationError`] for stale pair snapshots,
    /// credential or identity mismatches, invalid or gapped batch sequences,
    /// receipt conflicts, checkpoint violations, target and patch conflicts,
    /// budget and byte-bound violations, and counter overflow.
    pub async fn commit_run_batch(
        &self,
        command: CommitRunBatch<'_>,
    ) -> Result<CommitRunBatchOutcome, RunObservationError> {
        let digest = batch::validate_and_digest(&command)?;
        let transaction = self.begin_write().await.map_err(|source| {
            RunObservationError::Repository(database_error("begin run batch commit", source))
        })?;
        match execute_batch(&transaction, &command, &digest).await {
            Ok(BatchExecution::Persisted(info)) => {
                transaction.commit().await.map_err(|source| {
                    RunObservationError::Repository(database_error("commit run batch", source))
                })?;
                Ok(CommitRunBatchOutcome::Committed(info))
            }
            Ok(BatchExecution::Replay(info)) => {
                transaction.rollback().await.map_err(|source| {
                    RunObservationError::Repository(database_error(
                        "roll back run batch replay",
                        source,
                    ))
                })?;
                Ok(CommitRunBatchOutcome::AlreadyCommitted(info))
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    RunObservationError::Repository(database_error("roll back run batch", source))
                })?;
                Err(error)
            }
        }
    }
}

/// How the still-open transaction must be finished by the caller.
enum BatchExecution {
    /// All effects are staged; the caller commits once.
    Persisted(RunBatchReceiptInfo),
    /// An exact earlier commit was classified; the caller rolls back.
    Replay(RunBatchReceiptInfo),
}

async fn execute_batch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    digest: &[u8; 32],
) -> Result<BatchExecution, RunObservationError> {
    let dispatch_fenced = fence_dispatch(transaction, command).await?;
    if let Some(info) = classify_existing_receipt(transaction, command, digest).await? {
        return Ok(BatchExecution::Replay(info));
    }
    if !dispatch_fenced {
        return Err(classify_unfenced_dispatch(transaction, command).await);
    }
    if !fence_run(transaction, command).await? {
        return Err(classify_unfenced_run(transaction, command).await);
    }
    let context = load_batch_context(transaction, command).await?;
    let plan = build_plan(transaction, command, context, digest).await?;
    projection::persist_plan(transaction, plan, command.checkpoint).await?;
    let launched = command.scope.launched;
    Ok(BatchExecution::Persisted(RunBatchReceiptInfo {
        run_id: launched.run_id.clone(),
        generation: launched.generation,
        batch_sequence: command.batch_sequence,
    }))
}

/// Tentatively advances the RUNNING dispatch stamp; `false` means no row
/// matched the full pair snapshot.
async fn fence_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> Result<bool, RunObservationError> {
    let claimed = command.scope.claimed;
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        COMMIT_DISPATCH_SQL,
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
    let fenced = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("fence run batch dispatch", source))
        })?;
    Ok(fenced.is_some())
}

/// Classifies any persisted `(run_id, batch_sequence)` receipt inside the
/// serialized transaction. `Some` is an exact informational replay; a
/// diverging or uncommitted receipt is a typed conflict.
async fn classify_existing_receipt(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    digest: &[u8; 32],
) -> Result<Option<RunBatchReceiptInfo>, RunObservationError> {
    let launched = command.scope.launched;
    let receipt = entities::run_batch_receipt::Entity::find_by_id((
        launched.run_id.as_str().to_owned(),
        command.batch_sequence,
    ))
    .one(transaction)
    .await
    .map_err(|source| {
        RunObservationError::Repository(database_error("load run batch receipt", source))
    })?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    if !receipt.committed {
        return Err(RunObservationError::UncommittedReceipt {
            run_id: launched.run_id.clone(),
        });
    }
    if receipt.generation != launched.generation || receipt.digest.as_slice() != digest.as_slice() {
        return Err(RunObservationError::ReceiptConflict {
            run_id: launched.run_id.clone(),
        });
    }
    Ok(Some(RunBatchReceiptInfo {
        run_id: launched.run_id.clone(),
        generation: launched.generation,
        batch_sequence: command.batch_sequence,
    }))
}

/// Advances only the run's `updated_at_ms` under the full run fence;
/// `false` means no row matched.
async fn fence_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> Result<bool, RunObservationError> {
    let scope = &command.scope;
    let launched = scope.launched;
    let (owner_capability, lease_capability, _) = scope.credentials.parts();
    let statement = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        COMMIT_RUN_SQL,
        [
            millis(command.operated_at).into(),
            launched.run_id.as_str().into(),
            launched.thread_id.as_str().into(),
            launched.message_id.as_str().into(),
            launched.turn_id.as_str().into(),
            launched.generation.into(),
            scope.run_start_key.expose().to_vec().into(),
            owner_capability.expose().to_vec().into(),
            lease_capability.expose().to_vec().into(),
            millis(scope.expected_launch_at).into(),
            millis(scope.expected_updated_at).into(),
            scope.bound.binding_version.into(),
            millis(scope.bound.bound_at).into(),
        ],
    );
    let fenced = transaction
        .query_one_raw(statement)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("fence run batch run", source))
        })?;
    Ok(fenced.is_some())
}

/// Diagnoses a zero-row dispatch fence with no informational receipt.
async fn classify_unfenced_dispatch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> RunObservationError {
    let claimed = command.scope.claimed;
    let operated_at_ms = millis(command.operated_at);
    let dispatch = match entities::message_dispatch::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(dispatch)) => dispatch,
        Ok(None) => {
            return RunObservationError::Repository(RepositoryError::DispatchNotFound {
                message_id: claimed.message_id.clone(),
            });
        }
        Err(source) => {
            return RunObservationError::Repository(database_error(
                "classify unfenced batch dispatch",
                source,
            ));
        }
    };
    if dispatch.state != DispatchState::Running && dispatch.state != DispatchState::Leased {
        return RunObservationError::Repository(RepositoryError::InvalidDispatchState {
            message_id: claimed.message_id.clone(),
            state: dispatch_state_label(&dispatch.state),
        });
    }
    if let Some(expiry) = dispatch.lease_expires_at_ms
        && expiry <= operated_at_ms
    {
        return RunObservationError::Repository(RepositoryError::DispatchLeaseExpired {
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
        return RunObservationError::Repository(RepositoryError::DispatchOwnerMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    RunObservationError::SnapshotMismatch {
        message_id: claimed.message_id.clone(),
    }
}

/// Diagnoses a zero-row run fence after the dispatch fence matched.
async fn classify_unfenced_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> RunObservationError {
    let scope = &command.scope;
    let launched = scope.launched;
    let run = match entities::assistant_run::Entity::find_by_id(launched.run_id.as_str())
        .one(transaction)
        .await
    {
        Ok(Some(run)) => run,
        Ok(None) => {
            return RunObservationError::RunNotFound {
                run_id: launched.run_id.clone(),
            };
        }
        Err(source) => {
            return RunObservationError::Repository(database_error(
                "classify unfenced batch run",
                source,
            ));
        }
    };
    if run.lifecycle != AssistantRunLifecycle::Running {
        return RunObservationError::RunNotRunning {
            run_id: launched.run_id.clone(),
        };
    }
    if run.thread_id != launched.thread_id.as_str()
        || run.origin_message_id != launched.message_id.as_str()
        || run.origin_turn_id != launched.turn_id.as_str()
    {
        return RunObservationError::IdentityConflict {
            reason: "stored run originates from another thread, message, or turn",
        };
    }
    let (owner_capability, lease_capability, _) = scope.credentials.parts();
    if run.generation != launched.generation
        || !stored_bytes_match(&run.run_start_key, scope.run_start_key.expose())
        || !owner_capability.matches_stored(run.owner.as_ref())
        || !lease_capability.matches_stored(run.lease.as_ref())
        || run.claim_token.is_some()
        || run.provider_binding_version != Some(scope.bound.binding_version)
        || run.provider_binding.is_none()
        || run.provider_bound_at_ms != Some(millis(scope.bound.bound_at))
    {
        return RunObservationError::CredentialMismatch {
            run_id: launched.run_id.clone(),
        };
    }
    RunObservationError::SnapshotMismatch {
        message_id: scope.claimed.message_id.clone(),
    }
}

/// Loaded, validated in-transaction state for one fenced fresh batch.
struct BatchContext {
    state: projection::LoadedState,
    checkpoint_row: Option<entities::RunCheckpoint>,
    turn: projection::LoadedTurn,
}

async fn load_batch_context(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> Result<BatchContext, RunObservationError> {
    let operated_at_ms = millis(command.operated_at);
    let launched = command.scope.launched;
    let thread_id = launched.thread_id.as_str();

    let state = projection::load_conversation_state(transaction, thread_id)
        .await?
        .ok_or_else(|| {
            RunObservationError::Repository(corrupt_data(
                "conversation_state",
                "thread_id",
                "fenced run batch found no conversation state",
            ))
        })?;
    if state.next_renderer_ordinal < 0 {
        return Err(negative_counter("next_renderer_ordinal"));
    }
    if state.last_patch_sequence < 0 {
        return Err(negative_counter("last_patch_sequence"));
    }
    if operated_at_ms < state.updated_at_ms {
        return Err(chronology("conversation_state.updated_at_ms"));
    }

    let checkpoint_row =
        projection::load_run_checkpoint(transaction, launched.run_id.as_str()).await?;
    let last_batch_sequence =
        validate_checkpoint_row(transaction, command, checkpoint_row.as_ref()).await?;
    let expected_sequence =
        last_batch_sequence
            .checked_add(1)
            .ok_or(RunObservationError::CounterOverflow {
                counter: "batch sequence",
                value: last_batch_sequence,
            })?;
    if command.batch_sequence != expected_sequence {
        return Err(if command.batch_sequence > expected_sequence {
            RunObservationError::BatchSequenceGap {
                expected: expected_sequence,
                actual: command.batch_sequence,
            }
        } else {
            RunObservationError::InvalidBatchSequence {
                sequence: command.batch_sequence,
            }
        });
    }

    let turn = projection::load_turn(transaction, launched.turn_id.as_str())
        .await?
        .ok_or_else(|| {
            RunObservationError::Repository(corrupt_data(
                "conversation_turns",
                "turn_id",
                "fenced run batch lost its origin turn",
            ))
        })?;
    if turn.thread_id != thread_id {
        return Err(RunObservationError::Repository(corrupt_data(
            "conversation_turns",
            "thread_id",
            "origin turn belongs to another thread",
        )));
    }
    if operated_at_ms < turn.updated_at_ms {
        return Err(chronology("conversation_turns.updated_at_ms"));
    }
    validate_turn_activation(&turn, command)?;

    Ok(BatchContext {
        state,
        checkpoint_row,
        turn,
    })
}

/// Validates the checkpoint row against generation, chronology, and the
/// missing-row rule; returns the persisted `last_batch_sequence` (zero only
/// when no row and no receipts exist).
async fn validate_checkpoint_row(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    checkpoint_row: Option<&entities::RunCheckpoint>,
) -> Result<i64, RunObservationError> {
    let launched = command.scope.launched;
    let Some(row) = checkpoint_row else {
        if projection::receipts_exist_for_run(transaction, launched.run_id.as_str()).await? {
            return Err(RunObservationError::Repository(corrupt_data(
                "run_checkpoints",
                "run_id",
                "batch receipts exist without a checkpoint row",
            )));
        }
        return Ok(0);
    };
    if row.generation != launched.generation {
        return Err(RunObservationError::CheckpointGenerationMismatch {
            stored: row.generation,
            expected: launched.generation,
        });
    }
    if row.last_batch_sequence < 0 {
        return Err(RunObservationError::Repository(corrupt_data(
            "run_checkpoints",
            "last_batch_sequence",
            "counter is negative",
        )));
    }
    if millis(command.operated_at) < row.updated_at_ms {
        return Err(chronology("run_checkpoints.updated_at_ms"));
    }
    Ok(row.last_batch_sequence)
}

/// Requires the activation patch exactly when the origin turn is Pending and
/// rejects sealed turns outright.
fn validate_turn_activation(
    turn: &projection::LoadedTurn,
    command: &CommitRunBatch<'_>,
) -> Result<(), RunObservationError> {
    if matches!(
        turn.lifecycle,
        EntityLifecycle::Completed | EntityLifecycle::Failed | EntityLifecycle::Cancelled
    ) {
        return Err(RunObservationError::TargetConflict {
            reason: "origin turn is sealed",
        });
    }
    let pending = turn.lifecycle == EntityLifecycle::Pending;
    match (pending, command.activate_turn_patch_id.is_some()) {
        (true, false) => Err(RunObservationError::PatchConflict {
            reason: "pending turn requires its activation patch",
        }),
        (false, true) => Err(RunObservationError::PatchConflict {
            reason: "activation patch is forbidden after the first activation",
        }),
        _ => Ok(()),
    }
}

/// Mutable tentative-effect collection shared by the plan builders.
struct PlanAccumulator {
    next_ordinal: i64,
    patch_sequence: i64,
    fresh_ordinals: Vec<(i64, String)>,
    items_to_insert: Vec<projection::ItemRow>,
    items_to_update: Vec<projection::ItemRow>,
    patches: Vec<projection::PatchToInsert>,
}

/// Builds the complete tentative persistence plan in declared change order.
async fn build_plan(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    context: BatchContext,
    digest: &[u8; 32],
) -> Result<projection::PersistencePlan, RunObservationError> {
    let operated_at_ms = millis(command.operated_at);
    let launched = command.scope.launched;
    let mut accumulator = PlanAccumulator {
        next_ordinal: context.state.next_renderer_ordinal,
        patch_sequence: context.state.last_patch_sequence,
        fresh_ordinals: Vec::new(),
        items_to_insert: Vec::new(),
        items_to_update: Vec::new(),
        patches: Vec::new(),
    };

    let turn_update = if let Some(patch_id) = command.activate_turn_patch_id {
        ensure_patch_vacant(transaction, patch_id.as_str()).await?;
        let revision = next_revision("conversation_turns", context.turn.revision)?;
        accumulator.patch_sequence = next_counter(accumulator.patch_sequence, "patch sequence")?;
        accumulator.patches.push(turn_activation_patch(
            patch_id,
            accumulator.patch_sequence,
            revision,
            &context.turn,
            operated_at_ms,
        ));
        Some(projection::TurnRow {
            turn_id: context.turn.turn_id.clone(),
            thread_id: context.turn.thread_id.clone(),
            ordinal: context.turn.ordinal,
            revision,
            lifecycle: EntityLifecycle::Active,
            created_at_ms: context.turn.created_at_ms,
            updated_at_ms: operated_at_ms,
        })
    } else {
        None
    };

    for change in command.changes {
        plan_change(transaction, command, &mut accumulator, change).await?;
    }

    let ledger = build_ledger_inserts(transaction, command).await?;

    Ok(projection::PersistencePlan {
        thread_id: launched.thread_id.as_str().to_owned(),
        fresh_ordinals: accumulator.fresh_ordinals,
        items_to_insert: accumulator.items_to_insert,
        items_to_update: accumulator.items_to_update,
        turn_update,
        patches: accumulator.patches,
        state: projection::StateRow {
            next_renderer_ordinal: accumulator.next_ordinal,
            last_patch_sequence: accumulator.patch_sequence,
            updated_at_ms: operated_at_ms,
        },
        checkpoint: projection::CheckpointRow {
            existing: context.checkpoint_row,
            run_id: launched.run_id.as_str().to_owned(),
            generation: launched.generation,
            last_batch_sequence: command.batch_sequence,
            updated_at_ms: operated_at_ms,
        },
        receipt: projection::ReceiptRow {
            run_id: launched.run_id.as_str().to_owned(),
            generation: launched.generation,
            batch_sequence: command.batch_sequence,
            digest: *digest,
        },
        ledger,
    })
}

/// Extracts the claimed observation batch staged for the ledger, if any.
///
/// Returns `None` for `Keep` and for opaque non-observation checkpoints:
/// a checkpoint counts as claimed when its bytes parse as a JSON envelope
/// carrying [`OBSERVATION_FORMAT_TAG`], regardless of the outer checkpoint
/// version. Anything else is an unrelated opaque checkpoint that stages no
/// rows, preserving Replace semantics exactly. A claimed envelope always
/// goes through strict canonical decoding with its stored version, so a
/// claimed envelope with a mismatched version or corrupt body is a typed
/// rejection: the batch commits neither its checkpoint nor partial ledger
/// rows.
fn claimed_observation_batch(
    checkpoint: CheckpointUpdate<'_>,
) -> Result<Option<DecodedObservationBatch>, RunObservationError> {
    let CheckpointUpdate::Replace(engine) = checkpoint else {
        return Ok(None);
    };
    let bytes = engine.as_slice();
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(envelope) = value.as_object() else {
        return Ok(None);
    };
    let claimed = envelope.get("format").and_then(Value::as_str) == Some(OBSERVATION_FORMAT_TAG);
    if !claimed {
        return Ok(None);
    }
    decode_observation_checkpoint(engine.version(), bytes)
        .map(Some)
        .map_err(|_| RunObservationError::InvalidCheckpoint {
            reason: "claimed observation checkpoint is not canonical",
        })
}

/// Stages one immutable ledger row per claimed observation.
///
/// Delivery sequences allocate in-transaction across runs on the batch
/// thread; attribution resolves the Forge turn and commit instant from the
/// batch scope, never from a provider string. Each row payload is the
/// canonical single-observation envelope, reusing the batch codec without
/// duplicating its vocabulary.
async fn build_ledger_inserts(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
) -> Result<Vec<super::super::observation_ledger::LedgerInsert>, RunObservationError> {
    let Some(batch) = claimed_observation_batch(command.checkpoint)? else {
        return Ok(Vec::new());
    };
    let launched = command.scope.launched;
    let observations = batch.observations();
    let base = super::super::observation_ledger::allocate_delivery_base(
        transaction,
        launched.thread_id.as_str(),
        observations.len(),
    )
    .await?;
    let mut rows = Vec::with_capacity(observations.len());
    for (index, observation) in observations.iter().enumerate() {
        let offset = i64::try_from(index).map_err(|_| RunObservationError::CounterOverflow {
            counter: "delivery sequence",
            value: i64::MAX,
        })?;
        let delivery_sequence =
            base.checked_add(offset)
                .ok_or(RunObservationError::CounterOverflow {
                    counter: "delivery sequence",
                    value: base,
                })?;
        let observation_sequence = i64::try_from(observation.sequence().get()).map_err(|_| {
            RunObservationError::CounterOverflow {
                counter: "observation sequence",
                value: i64::MAX,
            }
        })?;
        let payload = encode_observation_bytes(
            batch.engine(),
            batch.binding_version(),
            None,
            std::slice::from_ref(observation),
        )
        .map_err(|_| RunObservationError::InvalidCheckpoint {
            reason: "observation ledger payload exceeds its byte ceiling",
        })?;
        rows.push(super::super::observation_ledger::LedgerInsert {
            thread_id: launched.thread_id.as_str().to_owned(),
            delivery_sequence,
            run_id: launched.run_id.as_str().to_owned(),
            observation_sequence,
            turn_id: launched.turn_id.as_str().to_owned(),
            committed_at_ms: millis(command.operated_at),
            engine: batch.engine().as_str().to_owned(),
            binding_version: batch.binding_version(),
            observation_version: OBSERVATION_CHECKPOINT_VERSION,
            observation_bytes: payload,
        });
    }
    Ok(rows)
}

/// Validates one declared change and stages its tentative effects.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive change table stages atomic item and patch updates"
)]
async fn plan_change(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    accumulator: &mut PlanAccumulator,
    change: &AssistantChange<'_>,
) -> Result<(), RunObservationError> {
    let operated_at_ms = millis(command.operated_at);
    let launched = command.scope.launched;
    match change {
        AssistantChange::Finish {
            item_id,
            expected_revision,
            patch_id,
        } => {
            let target =
                load_batch_target(transaction, command, item_id, *expected_revision).await?;
            ensure_patch_vacant(transaction, patch_id.as_str()).await?;
            let revision = next_revision("conversation_items", target.revision)?;
            accumulator.patch_sequence =
                next_counter(accumulator.patch_sequence, "patch sequence")?;
            let mut row =
                existing_item_row(&target, revision, target.body.clone(), operated_at_ms)?;
            row.lifecycle = EntityLifecycle::Completed;
            accumulator.patches.push(item_upsert_patch(
                patch_id,
                accumulator.patch_sequence,
                &row,
            ));
            accumulator.items_to_update.push(row);
        }
        AssistantChange::Start {
            item_id,
            phase,
            body,
            patch_id,
        } => {
            ensure_fresh_item_vacant(transaction, item_id.as_str()).await?;
            ensure_patch_vacant(transaction, patch_id.as_str()).await?;
            let ordinal = accumulator.next_ordinal;
            accumulator.next_ordinal = next_counter(ordinal, "renderer ordinal")?;
            accumulator.patch_sequence =
                next_counter(accumulator.patch_sequence, "patch sequence")?;
            accumulator
                .fresh_ordinals
                .push((ordinal, item_id.as_str().to_owned()));
            let row = projection::ItemRow {
                item_id: item_id.as_str().to_owned(),
                thread_id: launched.thread_id.as_str().to_owned(),
                turn_id: launched.turn_id.as_str().to_owned(),
                ordinal,
                revision: 0,
                lifecycle: EntityLifecycle::Streaming,
                phase: projection::map_phase(*phase),
                body: body.as_str().to_owned(),
                run_id: launched.run_id.as_str().to_owned(),
                created_at_ms: operated_at_ms,
                updated_at_ms: operated_at_ms,
            };
            accumulator.patches.push(item_upsert_patch(
                patch_id,
                accumulator.patch_sequence,
                &row,
            ));
            accumulator.items_to_insert.push(row);
        }
        AssistantChange::Append {
            item_id,
            expected_revision,
            text,
            patch_id,
        } => {
            let target =
                load_batch_target(transaction, command, item_id, *expected_revision).await?;
            ensure_patch_vacant(transaction, patch_id.as_str()).await?;
            let revision = next_revision("conversation_items", target.revision)?;
            accumulator.patch_sequence =
                next_counter(accumulator.patch_sequence, "patch sequence")?;
            let mut body = target.body.clone();
            body.push_str(text.as_str());
            if body.len() > AssistantBody::MAX_BYTES {
                return Err(RunObservationError::BodyTooLong {
                    length: body.len(),
                    maximum: AssistantBody::MAX_BYTES,
                });
            }
            accumulator.items_to_update.push(existing_item_row(
                &target,
                revision,
                body,
                operated_at_ms,
            )?);
            accumulator.patches.push(item_append_patch(
                patch_id,
                accumulator.patch_sequence,
                item_id.as_str(),
                revision,
                text.as_str(),
                operated_at_ms,
            ));
        }
        AssistantChange::Replace {
            item_id,
            expected_revision,
            body,
            phase,
            patch_id,
        } => {
            let target =
                load_batch_target(transaction, command, item_id, *expected_revision).await?;
            ensure_patch_vacant(transaction, patch_id.as_str()).await?;
            let revision = next_revision("conversation_items", target.revision)?;
            accumulator.patch_sequence =
                next_counter(accumulator.patch_sequence, "patch sequence")?;
            let mut row =
                existing_item_row(&target, revision, body.as_str().to_owned(), operated_at_ms)?;
            row.phase = projection::map_phase(*phase);
            accumulator.patches.push(item_upsert_patch(
                patch_id,
                accumulator.patch_sequence,
                &row,
            ));
            accumulator.items_to_update.push(row);
        }
    }
    Ok(())
}

/// Loads and fences one Append/Replace target through the transaction.
async fn load_batch_target(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    item_id: &ItemId,
    expected_revision: Revision,
) -> Result<entities::ConversationItem, RunObservationError> {
    let launched = command.scope.launched;
    let Some(item) = projection::load_item(transaction, item_id.as_str()).await? else {
        return Err(RunObservationError::TargetConflict {
            reason: "target item does not exist",
        });
    };
    if item.item_kind != ConversationItemKind::AssistantMessage {
        return Err(RunObservationError::TargetConflict {
            reason: "target item is not an assistant message",
        });
    }
    if item.thread_id != launched.thread_id.as_str()
        || item.turn_id != launched.turn_id.as_str()
        || item.run_id.as_deref() != Some(launched.run_id.as_str())
    {
        return Err(RunObservationError::TargetConflict {
            reason: "target item belongs to another run, turn, or thread",
        });
    }
    if matches!(
        item.lifecycle,
        EntityLifecycle::Completed | EntityLifecycle::Failed | EntityLifecycle::Cancelled
    ) {
        return Err(RunObservationError::SealedItem {
            item_id: item_id.clone(),
        });
    }
    if millis(command.operated_at) < item.updated_at_ms {
        return Err(chronology("conversation_items.updated_at_ms"));
    }
    let stored_revision = u64::try_from(item.revision).map_err(|_| {
        RunObservationError::Repository(corrupt_data(
            "conversation_items",
            "revision",
            "revision is negative",
        ))
    })?;
    if stored_revision != expected_revision.get() {
        return Err(RunObservationError::TargetConflict {
            reason: "expected revision does not match the stored item",
        });
    }
    Ok(item)
}

/// Full post-image of an existing assistant item with a new revision/body;
/// identity, ordinal, origin, lifecycle, phase, and `created_at_ms` are
/// preserved from the stored row.
fn existing_item_row(
    target: &entities::ConversationItem,
    revision: i64,
    body: String,
    operated_at_ms: i64,
) -> Result<projection::ItemRow, RunObservationError> {
    let phase = target.phase.clone().ok_or_else(|| {
        RunObservationError::Repository(corrupt_data(
            "conversation_items",
            "phase",
            "assistant item is missing its phase",
        ))
    })?;
    let run_id = target.run_id.clone().ok_or_else(|| {
        RunObservationError::Repository(corrupt_data(
            "conversation_items",
            "run_id",
            "assistant item is missing its run",
        ))
    })?;
    Ok(projection::ItemRow {
        item_id: target.item_id.clone(),
        thread_id: target.thread_id.clone(),
        turn_id: target.turn_id.clone(),
        ordinal: target.ordinal,
        revision,
        lifecycle: target.lifecycle.clone(),
        phase,
        body,
        run_id,
        created_at_ms: target.created_at_ms,
        updated_at_ms: operated_at_ms,
    })
}

fn turn_activation_patch(
    patch_id: &PatchId,
    sequence: i64,
    revision: i64,
    turn: &projection::LoadedTurn,
    operated_at_ms: i64,
) -> projection::PatchToInsert {
    projection::PatchToInsert {
        patch_id: patch_id.as_str().to_owned(),
        sequence,
        kind: entities::ConversationPatchKind::TurnLifecycle,
        revision,
        recorded_at_ms: operated_at_ms,
        turn_id: Some(turn.turn_id.clone()),
        item_id: None,
        ordinal: None,
        lifecycle: Some(EntityLifecycle::Active),
        item_kind: None,
        run_id: None,
        phase: None,
        body: None,
        fragment: None,
        entity_created_at_ms: None,
        entity_updated_at_ms: None,
    }
}

fn item_upsert_patch(
    patch_id: &PatchId,
    sequence: i64,
    row: &projection::ItemRow,
) -> projection::PatchToInsert {
    projection::PatchToInsert {
        patch_id: patch_id.as_str().to_owned(),
        sequence,
        kind: entities::ConversationPatchKind::ItemUpsert,
        revision: row.revision,
        recorded_at_ms: row.updated_at_ms,
        turn_id: Some(row.turn_id.clone()),
        item_id: Some(row.item_id.clone()),
        ordinal: Some(row.ordinal),
        lifecycle: Some(row.lifecycle.clone()),
        item_kind: Some(ConversationItemKind::AssistantMessage),
        run_id: Some(row.run_id.clone()),
        phase: Some(row.phase.clone()),
        body: Some(row.body.clone()),
        fragment: None,
        entity_created_at_ms: Some(row.created_at_ms),
        entity_updated_at_ms: Some(row.updated_at_ms),
    }
}

fn item_append_patch(
    patch_id: &PatchId,
    sequence: i64,
    item_id: &str,
    revision: i64,
    fragment: &str,
    operated_at_ms: i64,
) -> projection::PatchToInsert {
    projection::PatchToInsert {
        patch_id: patch_id.as_str().to_owned(),
        sequence,
        kind: entities::ConversationPatchKind::ItemAppend,
        revision,
        recorded_at_ms: operated_at_ms,
        turn_id: None,
        item_id: Some(item_id.to_owned()),
        ordinal: None,
        lifecycle: None,
        item_kind: None,
        run_id: None,
        phase: None,
        body: None,
        fragment: Some(fragment.to_owned()),
        entity_created_at_ms: None,
        entity_updated_at_ms: None,
    }
}

/// Rejects a fresh Start identity that collides with any persisted item or
/// ordinal-ledger entity.
async fn ensure_fresh_item_vacant(
    transaction: &sea_orm::DatabaseTransaction,
    item_id: &str,
) -> Result<(), RunObservationError> {
    if projection::load_item(transaction, item_id).await?.is_some() {
        return Err(RunObservationError::PatchConflict {
            reason: "fresh item identity already exists",
        });
    }
    if projection::ordinal_entity_exists(transaction, item_id).await? {
        return Err(RunObservationError::PatchConflict {
            reason: "fresh item identity already owns a renderer ordinal",
        });
    }
    Ok(())
}

/// Rejects a supplied patch identity that already exists durably.
async fn ensure_patch_vacant(
    transaction: &sea_orm::DatabaseTransaction,
    patch_id: &str,
) -> Result<(), RunObservationError> {
    if projection::patch_exists(transaction, patch_id).await? {
        return Err(RunObservationError::PatchConflict {
            reason: "patch identity already exists",
        });
    }
    Ok(())
}

/// Advances one signed counter without wraparound.
fn next_counter(value: i64, counter: &'static str) -> Result<i64, RunObservationError> {
    value
        .checked_add(1)
        .ok_or(RunObservationError::CounterOverflow { counter, value })
}

/// Advances one persisted revision through both its signed persisted and
/// unsigned domain representations without wraparound.
fn next_revision(table: &'static str, current: i64) -> Result<i64, RunObservationError> {
    let domain = u64::try_from(current).map_err(|_| {
        RunObservationError::Repository(corrupt_data(table, "revision", "revision is negative"))
    })?;
    let advanced =
        Revision::new(domain)
            .checked_next()
            .map_err(|_| RunObservationError::CounterOverflow {
                counter: "revision",
                value: current,
            })?;
    i64::try_from(advanced.get()).map_err(|_| RunObservationError::CounterOverflow {
        counter: "revision",
        value: current,
    })
}

fn chronology(earlier_field: &'static str) -> RunObservationError {
    RunObservationError::Repository(RepositoryError::InvalidChronology {
        earlier_field,
        later_field: "batch operated_at",
    })
}

fn negative_counter(column: &'static str) -> RunObservationError {
    RunObservationError::Repository(corrupt_data(
        "conversation_state",
        column,
        "counter is negative",
    ))
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

// ---------------------------------------------------------------------------
// S1a: version-tagged typed observation checkpoint codec
// ---------------------------------------------------------------------------
//
// Typed engine observations ride the existing batch payload: [`encode_observation_checkpoint`]
// packs one bounded, engine-tagged, monotonically sequenced batch into an
// [`EngineCheckpoint`] with the explicit format tag [`OBSERVATION_FORMAT_TAG`],
// exactly like the engine run config codec packs typed selections into
// version-tagged JSON. Callers commit the checkpoint through the existing
// [`Repository::commit_run_batch`] path with [`CheckpointUpdate::Replace`];
// the checkpoint row still keeps only the latest batch per run, and the
// append-only observation ledger (sibling `observation_ledger` module plus
// its migration) preserves every committed batch as immutable per-row
// history in the same transaction. The database tests prove the checkpoint
// half by committing fixture observations end to end and decoding the
// persisted `run_checkpoints` row.
//
// No `serde` derives leak into the domain crate: canonical encoding lives on
// the private `Stored*` structs below, strict decoding is manual with exact
// key sets per tag (mirroring `deny_unknown_fields`), and every rejection is
// a payload-free [`ObservationCommitError`].
