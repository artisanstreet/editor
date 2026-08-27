//! Atomic RUNNING assistant progress/checkpoint commit.
//!
//! [`Repository::commit_run_batch`] is the E2-A transactional boundary. One
//! transaction advances the RUNNING dispatch stamp and the running assistant
//! run together with the ordinal ledger, fresh or updated assistant items,
//! correctly ordered replay patches, the conversation-state counters, the
//! run checkpoint, and the `committed = true` receipt carrying the canonical
//! v1 digest. There is exactly one commit; every other outcome — replay
//! classification included — explicitly rolls the transaction back.
//!
//! Replay is informational only. An exact replay of an already committed
//! batch answers [`CommitRunBatchOutcome::AlreadyCommitted`] with identities
//! and the batch sequence, never a current cursor, execution permission, or
//! provider authority. The dispatch claim token was erased when the run was
//! bound, so the claim-token component of the supplied credentials is
//! deliberately unverifiable here: it is ignored rather than misrepresented
//! as checked. This method starts no delivery task and no notifier.

mod batch;
mod projection;

use artisan_domain::{
    AssistantBody, AssistantMessagePhase, IncrementalText, ItemId, MessageId, PatchId, Revision,
    RunId, UnixMillis,
};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, TransactionTrait};
use thiserror::Error;
use zeroize::Zeroize;

use crate::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, DispatchState, EntityLifecycle,
};

use super::message_dispatch::DispatchLeaseOwner;
use super::run_binding::BoundRunReceipt;
use super::run_launch::{
    LaunchedRunReceipt, RunLaunchCredentials, RunStartKey, stored_bytes_match,
};
use super::{
    ClaimedMessageDispatch, Repository, RepositoryError, corrupt_data, database_error, millis,
};

/// Inclusive engine-checkpoint payload bounds mirrored from the schema CHECK.
const ENGINE_CHECKPOINT_MIN_BYTES: usize = 1;
const ENGINE_CHECKPOINT_MAX_BYTES: usize = 262_144;

const COMMIT_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET updated_at_ms = ?
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

const COMMIT_RUN_SQL: &str = r"
UPDATE assistant_runs
SET updated_at_ms = ?
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

/// Validated opaque engine-checkpoint payload.
///
/// The wrapper enforces a positive version and 1..=262144 payload bytes,
/// implements neither formatting nor duplication traits, exposes no public
/// raw-byte accessor, and zeroizes its bytes on drop. Persisted form uses the
/// redacted `OpaqueBytes` model type so checkpoint bytes never appear in
/// public API results or model `Debug` output.
pub struct EngineCheckpoint {
    version: i64,
    bytes: Vec<u8>,
}

impl EngineCheckpoint {
    /// Creates a validated checkpoint payload without truncation.
    ///
    /// # Errors
    ///
    /// Returns [`RunObservationError::InvalidCheckpoint`] when `version` is
    /// not positive or `bytes` is empty or exceeds 262144 bytes.
    pub fn new(version: i64, bytes: Vec<u8>) -> Result<Self, RunObservationError> {
        if version <= 0 {
            return Err(RunObservationError::InvalidCheckpoint {
                reason: "checkpoint version must be positive",
            });
        }
        let length = bytes.len();
        if !(ENGINE_CHECKPOINT_MIN_BYTES..=ENGINE_CHECKPOINT_MAX_BYTES).contains(&length) {
            return Err(RunObservationError::InvalidCheckpoint {
                reason: "checkpoint payload must be 1..=262144 bytes",
            });
        }
        Ok(Self { version, bytes })
    }

    pub(super) const fn version(&self) -> i64 {
        self.version
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for EngineCheckpoint {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Whether one batch keeps or replaces the persisted engine-checkpoint tuple.
///
/// `Keep` preserves the stored tuple (or leaves it NULL for a first row);
/// `Replace` writes the version and payload together. There is deliberately
/// no `Clear` and no checkpoint-only batch. The enum implements no
/// formatting trait so it can never leak the referenced payload.
#[derive(Clone, Copy)]
pub enum CheckpointUpdate<'a> {
    /// Preserve the persisted engine tuple exactly as stored.
    Keep,
    /// Replace the persisted engine tuple with this validated payload.
    Replace(&'a EngineCheckpoint),
}

/// Closed RUN-SCOPED assistant mutation vocabulary applied in declared order.
#[derive(Clone, Copy)]
pub enum AssistantChange<'a> {
    /// Creates a fresh Streaming assistant item at revision zero with a fresh
    /// renderer ordinal allocated from `conversation_state`.
    Start {
        /// Caller-minted identity of the fresh assistant item.
        item_id: &'a ItemId,
        /// Renderer-disclosed text phase of the opening body.
        phase: AssistantMessagePhase,
        /// Complete opening body; empty and whitespace-only text are valid.
        body: &'a AssistantBody,
        /// Caller-minted identity of the emitted `item_upsert` patch.
        patch_id: &'a PatchId,
    },
    /// Appends one bounded fragment to an unsealed assistant item.
    Append {
        /// Target assistant item owned by this run.
        item_id: &'a ItemId,
        /// Revision the caller observed; must equal the stored revision.
        expected_revision: Revision,
        /// Exact incremental fragment; empty is permitted.
        text: &'a IncrementalText,
        /// Caller-minted identity of the emitted `item_append` patch.
        patch_id: &'a PatchId,
    },
    /// Replaces the complete body and phase of an unsealed assistant item;
    /// this is the `item_upsert` seam for settled or corrected text.
    Replace {
        /// Target assistant item owned by this run.
        item_id: &'a ItemId,
        /// Revision the caller observed; must equal the stored revision.
        expected_revision: Revision,
        /// Complete replacement body.
        body: &'a AssistantBody,
        /// Renderer-disclosed text phase after the replacement.
        phase: AssistantMessagePhase,
        /// Caller-minted identity of the emitted `item_upsert` patch.
        patch_id: &'a PatchId,
    },
}

/// Borrowed scope binding one batch to its claimed pair and launched run.
pub struct RunBatchScope<'a> {
    /// The original claim snapshot returned by `claim_next_message_dispatch`.
    pub claimed: &'a ClaimedMessageDispatch,
    /// Durable launch receipt carrying run/thread/message/turn/generation
    /// identities. Its USER item and `resulting_cursor` are historical launch
    /// facts, never current batch counters.
    pub launched: &'a LaunchedRunReceipt,
    /// Durable binding receipt; all nonsecret identities, `binding_version`,
    /// and `bound_at` are compared with the persisted run. The receipt is
    /// never a capability.
    pub bound: &'a BoundRunReceipt,
    /// Exact 32-byte deduplication key of the launched run.
    pub run_start_key: &'a RunStartKey,
    /// Named owner/lease/claim capabilities of the launched run. Binding
    /// erased the persisted claim token, so the claim component is
    /// deliberately unverifiable here and is ignored, never pretended
    /// verified.
    pub credentials: &'a RunLaunchCredentials,
    /// Original launch `operated_at` (the run's `created_at_ms`).
    pub expected_launch_at: UnixMillis,
    /// Exact previous successful pair stamp: `bound.bound_at` for the first
    /// batch, thereafter the previous successful commit's `operated_at`.
    pub expected_updated_at: UnixMillis,
}

/// Borrowed inputs of one atomic progress/checkpoint batch commit.
pub struct CommitRunBatch<'a> {
    /// Pair scope and credentials for the fenced transaction.
    pub scope: RunBatchScope<'a>,
    /// Strictly positive sequence; must equal the persisted
    /// `last_batch_sequence + 1`.
    pub batch_sequence: i64,
    /// Caller-injected operation time; no internal clock and no TTL.
    pub operated_at: UnixMillis,
    /// Required exactly when this batch performs the first Pending→Active
    /// turn activation; forbidden otherwise.
    pub activate_turn_patch_id: Option<&'a PatchId>,
    /// Nonempty run-scoped mutations applied in declared order.
    pub changes: &'a [AssistantChange<'a>],
    /// Keep or replace the persisted engine-checkpoint tuple.
    pub checkpoint: CheckpointUpdate<'a>,
}

/// Payload-free durable receipt information of one committed batch.
///
/// Carries identities and the batch sequence only — no historical cursor and
/// no execution permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBatchReceiptInfo {
    /// Run the batch belongs to.
    pub run_id: RunId,
    /// Generation recorded on the receipt.
    pub generation: i64,
    /// Committed batch sequence.
    pub batch_sequence: i64,
}

/// Typed outcome of one batch commit call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitRunBatchOutcome {
    /// This transaction committed the batch.
    Committed(RunBatchReceiptInfo),
    /// An earlier identical transaction already committed exactly this batch;
    /// informational receipt data only, never authority to write again or to
    /// reissue an external prompt.
    AlreadyCommitted(RunBatchReceiptInfo),
}

/// Capability-specific failures of [`Repository::commit_run_batch`].
///
/// Existing repository-layer rejections surface through
/// [`RunObservationError::Repository`] with their original typed source.
/// No variant carries secret bytes, checkpoint bytes, or body text.
#[derive(Debug, Error)]
pub enum RunObservationError {
    /// The supplied run identity does not exist.
    #[error("run `{run_id}` does not exist")]
    RunNotFound {
        /// Supplied run identity.
        run_id: RunId,
    },
    /// The run is not in its running lifecycle.
    #[error("run `{run_id}` is not in running state")]
    RunNotRunning {
        /// Supplied run identity.
        run_id: RunId,
    },
    /// A start key, capability, generation, or binding metadatum mismatched.
    #[error("run `{run_id}` credential or binding metadata did not match")]
    CredentialMismatch {
        /// Supplied run identity.
        run_id: RunId,
    },
    /// The supplied pair snapshot no longer describes persisted state.
    #[error("claimed dispatch snapshot for `{message_id}` no longer matches")]
    SnapshotMismatch {
        /// Claimed message identity.
        message_id: MessageId,
    },
    /// A colliding or contradictory run identity was supplied.
    #[error("run identity conflict: {reason}")]
    IdentityConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// A batch must carry at least one assistant change.
    #[error("run batch changes must not be empty")]
    EmptyBatch,
    /// The supplied batch sequence is not the next fresh sequence.
    #[error("batch sequence {sequence} is invalid")]
    InvalidBatchSequence {
        /// Offending sequence.
        sequence: i64,
    },
    /// The supplied batch sequence skipped ahead of the persisted counter.
    #[error("batch sequence gap: expected {expected}, received {actual}")]
    BatchSequenceGap {
        /// Next contiguous sequence required.
        expected: i64,
        /// Later sequence that exposed the gap.
        actual: i64,
    },
    /// A persisted receipt for this sequence contradicts the command.
    #[error("batch receipt for run `{run_id}` conflicts with this command")]
    ReceiptConflict {
        /// Run whose receipt conflicted.
        run_id: RunId,
    },
    /// A persisted receipt for this sequence was never committed.
    #[error("batch receipt for run `{run_id}` was never committed")]
    UncommittedReceipt {
        /// Run whose receipt is uncommitted.
        run_id: RunId,
    },
    /// The supplied checkpoint tuple violates its bounds.
    #[error("invalid engine checkpoint: {reason}")]
    InvalidCheckpoint {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// The persisted checkpoint generation contradicts the run generation.
    #[error("checkpoint generation {stored} does not match run generation {expected}")]
    CheckpointGenerationMismatch {
        /// Generation stored on the checkpoint row.
        stored: i64,
        /// Generation the command carries.
        expected: i64,
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
    /// A patch or fresh-item identity collides within the call or durably.
    #[error("patch identity conflict: {reason}")]
    PatchConflict {
        /// Bounded payload-free reason label.
        reason: &'static str,
    },
    /// The batch would emit more patches than the domain replay bound.
    #[error("run batch emits {count} patches; the maximum is {maximum}")]
    PatchBudgetExceeded {
        /// Offending emitted patch count including any activation patch.
        count: usize,
        /// The domain patch-batch ceiling.
        maximum: usize,
    },
    /// A resulting assistant body exceeded its byte ceiling.
    #[error("assistant body would be {length} UTF-8 bytes; the maximum is {maximum}")]
    BodyTooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The shared body ceiling.
        maximum: usize,
    },
    /// A supplied fragment exceeded its byte ceiling.
    #[error("text fragment is {length} UTF-8 bytes; the maximum is {maximum}")]
    FragmentTooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The fragment ceiling.
        maximum: usize,
    },
    /// A persisted counter could not advance within its checked range.
    #[error("{counter} counter overflowed at {value}")]
    CounterOverflow {
        /// Counter that could not advance.
        counter: &'static str,
        /// Value at the boundary.
        value: i64,
    },
    /// An existing repository rejection surfaced unchanged.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

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
    /// once. Any failure explicitly rolls back everything including the
    /// tentative fences. A commit error has unknown outcome: the caller may
    /// retry the exact command for receipt classification but must never
    /// reissue an external prompt.
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
        let transaction = self.database.begin().await.map_err(|source| {
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
    })
}

/// Validates one declared change and stages its tentative effects.
async fn plan_change(
    transaction: &sea_orm::DatabaseTransaction,
    command: &CommitRunBatch<'_>,
    accumulator: &mut PlanAccumulator,
    change: &AssistantChange<'_>,
) -> Result<(), RunObservationError> {
    let operated_at_ms = millis(command.operated_at);
    let launched = command.scope.launched;
    match change {
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
