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
pub mod terminal;

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
) -> Result<Vec<super::observation_ledger::LedgerInsert>, RunObservationError> {
    let Some(batch) = claimed_observation_batch(command.checkpoint)? else {
        return Ok(Vec::new());
    };
    let launched = command.scope.launched;
    let observations = batch.observations();
    let base = super::observation_ledger::allocate_delivery_base(
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
        rows.push(super::observation_ledger::LedgerInsert {
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

use serde::Serialize;
use serde_json::{Map, Value};

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalKind,
    ApprovalObservation, ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation,
    CompactionState, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, EngineId, FileAction,
    FileObservation, LimitScope, MessagePhase, NativeActionObservation, Observation,
    ObservationError, ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus,
    PlanObservation, ProcessDiagnosticObservation, ProtocolDiagnosticObservation, QuestionInput,
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

/// Engine checkpoint version carrying a typed observation batch.
pub const OBSERVATION_CHECKPOINT_VERSION: i64 = 1;

/// Explicit format tag of every observation checkpoint payload.
pub const OBSERVATION_FORMAT_TAG: &str = "artisan.observation.v1";

/// Maximum observations in one committed batch.
///
/// Mirrors the conversation patch batch ceiling so one commit stays bounded
/// end to end.
pub const OBSERVATION_BATCH_MAX_OBSERVATIONS: usize = 64;

/// Typed failures of the observation checkpoint codec and bind agreement.
///
/// No variant carries observation text, identities, or provider payloads;
/// counts and lengths are bounded numbers only.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ObservationCommitError {
    /// The batch carried no observations.
    #[error("observation batch must carry at least one observation")]
    EmptyBatch,
    /// The batch exceeded its documented entry ceiling.
    #[error("observation batch has {count} observations; the maximum is {maximum}")]
    TooMany {
        /// Offending observation count.
        count: usize,
        /// The documented batch ceiling.
        maximum: usize,
    },
    /// The encoded payload exceeded the engine checkpoint byte ceiling.
    #[error("observation checkpoint is {length} bytes; the maximum is {maximum}")]
    TooLarge {
        /// Offending length in bytes.
        length: usize,
        /// The checkpoint byte ceiling.
        maximum: usize,
    },
    /// The binding version did not match the bound run.
    #[error("observation binding version does not match the bound run")]
    BindMismatch,
    /// The engine tag did not match the previously committed batch.
    #[error("observation engine tag does not match the committed batch")]
    EngineMismatch,
    /// The checkpoint version is not the observation version.
    #[error("observation checkpoint version does not match")]
    VersionMismatch,
    /// The checkpoint format tag is not the observation tag.
    #[error("observation checkpoint format tag does not match")]
    FormatMismatch,
    /// Observation sequences were not strictly increasing.
    #[error("observation sequences are not strictly increasing")]
    SequenceNotMonotonic,
    /// An observation tag is not a modeled engine observation.
    #[error("observation tag is unknown")]
    UnknownObservation,
    /// An engine tag is not a modeled engine.
    #[error("observation engine tag is unknown")]
    UnknownEngine,
    /// The checkpoint bytes are not a JSON observation envelope.
    #[error("observation checkpoint bytes are malformed")]
    Malformed,
    /// The checkpoint bytes decode but are not the canonical encoding.
    #[error("observation checkpoint bytes are not canonical")]
    NonCanonical,
    /// The batch could not be encoded.
    #[error("observation checkpoint could not be encoded")]
    Encode,
    /// The encoded payload failed checkpoint validation.
    #[error("observation checkpoint payload is invalid")]
    InvalidCheckpoint,
    /// One observation value violated its domain bounds.
    #[error(transparent)]
    InvalidObservation(#[from] ObservationError),
}

/// One decoded, engine-tagged observation batch.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedObservationBatch {
    engine: EngineId,
    binding_version: i64,
    observations: Vec<Observation>,
}

impl DecodedObservationBatch {
    /// Returns the engine tag the batch was committed under.
    #[must_use]
    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    /// Returns the binding version the batch was committed under.
    #[must_use]
    pub const fn binding_version(&self) -> i64 {
        self.binding_version
    }

    /// Returns the decoded observations in durable sequence order.
    #[must_use]
    pub const fn observations(&self) -> &Vec<Observation> {
        &self.observations
    }

    /// Returns the greatest durable sequence in the batch, if any.
    #[must_use]
    pub fn max_sequence(&self) -> Option<u64> {
        self.observations
            .iter()
            .map(|observation| observation.sequence().get())
            .max()
    }
}

/// Encodes one bounded observation batch to its canonical bytes.
///
/// This is the byte-level form of [`encode_observation_checkpoint`]: same
/// validation and the same canonical layout, without the checkpoint wrapper.
/// Tests and tooling use it to verify canonical bytes; production commits wrap
/// it in an [`EngineCheckpoint`] so checkpoint bytes stay redacted.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for an empty or oversized batch,
/// non-positive binding versions, non-monotonic sequences, oversized payloads,
/// or domain bound violations.
pub fn encode_observation_bytes(
    engine: EngineId,
    binding_version: i64,
    base_sequence: Option<u64>,
    observations: &[Observation],
) -> Result<Vec<u8>, ObservationCommitError> {
    if binding_version <= 0 {
        return Err(ObservationCommitError::InvalidObservation(
            ObservationError::OutOfRange {
                field: "binding_version",
            },
        ));
    }
    if observations.is_empty() {
        return Err(ObservationCommitError::EmptyBatch);
    }
    if observations.len() > OBSERVATION_BATCH_MAX_OBSERVATIONS {
        return Err(ObservationCommitError::TooMany {
            count: observations.len(),
            maximum: OBSERVATION_BATCH_MAX_OBSERVATIONS,
        });
    }
    let mut previous = base_sequence;
    for observation in observations {
        let sequence = observation.sequence().get();
        if previous.is_some_and(|bound| sequence <= bound) {
            return Err(ObservationCommitError::SequenceNotMonotonic);
        }
        previous = Some(sequence);
    }
    let encoded = encode_bytes(engine, binding_version, observations)?;
    if encoded.len() > ENGINE_CHECKPOINT_MAX_BYTES {
        return Err(ObservationCommitError::TooLarge {
            length: encoded.len(),
            maximum: ENGINE_CHECKPOINT_MAX_BYTES,
        });
    }
    Ok(encoded)
}

/// Packs one bounded observation batch into an engine checkpoint.
///
/// Sequences must be strictly increasing and strictly greater than
/// `base_sequence` (the previously committed maximum, [`None`] for the first
/// batch of a run). Bind agreement is checked separately with
/// [`validate_observation_bind`]; the checkpoint embeds the engine tag and
/// binding version so durable history stays attributable.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for an empty or oversized batch,
/// non-positive binding versions, non-monotonic sequences, oversized payloads,
/// or domain bound violations. No SQL is opened here; commit the returned
/// checkpoint through [`Repository::commit_run_batch`].
pub fn encode_observation_checkpoint(
    engine: EngineId,
    binding_version: i64,
    base_sequence: Option<u64>,
    observations: &[Observation],
) -> Result<EngineCheckpoint, ObservationCommitError> {
    let encoded = encode_observation_bytes(engine, binding_version, base_sequence, observations)?;
    EngineCheckpoint::new(OBSERVATION_CHECKPOINT_VERSION, encoded)
        .map_err(|_| ObservationCommitError::InvalidCheckpoint)
}

/// Decodes one persisted observation checkpoint and proves canonicality.
///
/// The caller supplies the stored checkpoint version and blob (for example
/// from the `run_checkpoints` row written by [`Repository::commit_run_batch`])
/// and receives the engine tag, binding version, and validated observations.
/// Unknown engines, tags, and provider values reject with typed errors.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for version, format, engine, sequence,
/// bound, shape, and canonicality violations.
pub fn decode_observation_checkpoint(
    version: i64,
    bytes: &[u8],
) -> Result<DecodedObservationBatch, ObservationCommitError> {
    if version != OBSERVATION_CHECKPOINT_VERSION {
        return Err(ObservationCommitError::VersionMismatch);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| ObservationCommitError::Malformed)?;
    let envelope = value.as_object().ok_or(ObservationCommitError::Malformed)?;
    require_keys(
        envelope,
        &[
            "format",
            "version",
            "engine",
            "binding_version",
            "observations",
        ],
    )?;
    if get_str(envelope, "format")? != OBSERVATION_FORMAT_TAG {
        return Err(ObservationCommitError::FormatMismatch);
    }
    if get_i64(envelope, "version")? != OBSERVATION_CHECKPOINT_VERSION {
        return Err(ObservationCommitError::VersionMismatch);
    }
    let engine = EngineId::parse(get_str(envelope, "engine")?)
        .map_err(|_| ObservationCommitError::UnknownEngine)?;
    let binding_version = get_i64(envelope, "binding_version")?;
    if binding_version <= 0 {
        return Err(ObservationCommitError::InvalidObservation(
            ObservationError::OutOfRange {
                field: "binding_version",
            },
        ));
    }
    let raw = envelope
        .get("observations")
        .and_then(Value::as_array)
        .ok_or(ObservationCommitError::Malformed)?;
    if raw.is_empty() {
        return Err(ObservationCommitError::EmptyBatch);
    }
    if raw.len() > OBSERVATION_BATCH_MAX_OBSERVATIONS {
        return Err(ObservationCommitError::TooMany {
            count: raw.len(),
            maximum: OBSERVATION_BATCH_MAX_OBSERVATIONS,
        });
    }
    let mut observations = Vec::with_capacity(raw.len());
    for entry in raw {
        let object = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
        observations.push(decode_observation(object)?);
    }
    let canonical = encode_bytes(engine, binding_version, &observations)?;
    if canonical.as_slice() != bytes {
        return Err(ObservationCommitError::NonCanonical);
    }
    Ok(DecodedObservationBatch {
        engine,
        binding_version,
        observations,
    })
}

/// Requires an observation batch to agree with its run bind.
///
/// The binding version must equal the bound receipt's version: observations
/// commit only under the bind they were produced for, never across a rebind.
///
/// # Errors
///
/// Returns [`ObservationCommitError::BindMismatch`] on any version divergence.
pub fn validate_observation_bind(
    binding_version: i64,
    bound: &BoundRunReceipt,
) -> Result<(), ObservationCommitError> {
    if binding_version != bound.binding_version {
        return Err(ObservationCommitError::BindMismatch);
    }
    Ok(())
}

/// Requires a decoded batch to continue the committed engine history.
///
/// The engine tag of a newly decoded batch must equal the expected engine so
/// a run's durable observation history never mixes engine vocabularies.
///
/// # Errors
///
/// Returns [`ObservationCommitError::EngineMismatch`] on any tag divergence.
pub fn validate_observation_engine(
    engine: EngineId,
    batch: &DecodedObservationBatch,
) -> Result<(), ObservationCommitError> {
    if batch.engine != engine {
        return Err(ObservationCommitError::EngineMismatch);
    }
    Ok(())
}

fn encode_bytes(
    engine: EngineId,
    binding_version: i64,
    observations: &[Observation],
) -> Result<Vec<u8>, ObservationCommitError> {
    let mut stored = Vec::with_capacity(observations.len());
    for observation in observations {
        stored.push(stored_observation(observation));
    }
    let batch = StoredBatch {
        format: OBSERVATION_FORMAT_TAG,
        version: OBSERVATION_CHECKPOINT_VERSION,
        engine: engine.as_str(),
        binding_version,
        observations: stored,
    };
    serde_json::to_vec(&batch).map_err(|_| ObservationCommitError::Encode)
}

// ---------------------------------------------------------------------------
// Canonical encoding structs (field order is the persisted contract)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StoredBatch<'a> {
    format: &'static str,
    version: i64,
    engine: &'static str,
    binding_version: i64,
    observations: Vec<StoredObservation<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StoredObservation<'a> {
    AgentMessageDelta(StoredAgentMessageDelta<'a>),
    AgentMessageCompleted(StoredAgentMessageCompleted<'a>),
    Approval(StoredApproval<'a>),
    Compaction(StoredCompaction<'a>),
    File(StoredFile<'a>),
    NativeAction(StoredNativeAction<'a>),
    Plan(StoredPlan<'a>),
    ProcessDiagnostic(StoredProcessDiagnostic<'a>),
    ProtocolDiagnostic(StoredProtocolDiagnostic<'a>),
    Question(StoredQuestion<'a>),
    ReasoningSummaryCompleted(StoredReasoningSummaryCompleted<'a>),
    ReasoningSummaryDelta(StoredReasoningSummaryDelta<'a>),
    Retry(StoredRetry<'a>),
    RunState(StoredRunState<'a>),
    RunTerminal(StoredRunTerminal<'a>),
    Search(StoredSearch<'a>),
    Subagent(StoredSubagent<'a>),
    SubagentTranscript(StoredSubagentTranscript<'a>),
    TerminalActivity(StoredTerminalActivity<'a>),
    Tool(StoredTool<'a>),
    TurnState(StoredTurnState<'a>),
    Usage(StoredUsage<'a>),
}

#[derive(Serialize)]
struct StoredAgentMessageDelta<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    phase: &'static str,
    delta: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredAgentMessageCompleted<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    phase: &'static str,
    message: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredReasoningSummaryDelta<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    summary_index: u64,
    delta: &'a str,
    thinking_tokens: Option<u64>,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredReasoningSummaryCompleted<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    text: Option<&'a str>,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredTool<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    tool_id: &'a str,
    tool_name: &'a str,
    action: &'static str,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredFile<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    path: &'a str,
    action: &'static str,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

#[derive(Serialize)]
struct StoredSearch<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    query: &'a str,
    scope: Option<&'static str>,
    search_id: Option<&'a str>,
    state: &'static str,
    result_count: Option<u64>,
}

#[derive(Serialize)]
struct StoredTerminalActivity<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    activity_id: &'a str,
    channel: Option<&'static str>,
    command: Option<&'a str>,
    shell: Option<&'a str>,
    output: Option<&'a str>,
    exit_code: Option<i32>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredApprovalRequest<'a> {
    kind: &'static str,
    command: Option<&'a str>,
    cwd: Option<&'a str>,
    reason: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredApproval<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    approval_id: &'a str,
    state: &'static str,
    description: &'a str,
    request: StoredApprovalRequest<'a>,
    approved: Option<bool>,
}

#[derive(Serialize)]
struct StoredQuestionOption<'a> {
    label: &'a str,
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredQuestion<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    question_id: &'a str,
    state: &'static str,
    text: &'a str,
    header: Option<&'a str>,
    multi_select: bool,
    options: Option<Vec<StoredQuestionOption<'a>>>,
    answers: Option<&'a Vec<String>>,
}

#[derive(Serialize)]
struct StoredPlanEntry<'a> {
    id: &'a str,
    status: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct StoredPlan<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    entries: Vec<StoredPlanEntry<'a>>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredCompaction<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
    compaction_id: Option<&'a str>,
    duration_ms: Option<u64>,
    summary: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredRetry<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    turn_id: &'a str,
    attempt_state: &'static str,
    will_retry: bool,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredRunState<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredTurnState<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    turn_id: &'a str,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredSubagent<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    agent_native_thread_id: &'a str,
    parent_native_thread_id: &'a str,
    state: &'static str,
    activity: Option<&'a str>,
    agent_path: Option<&'a str>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StoredTranscriptContent<'a> {
    AgentMessageDelta(StoredTranscriptAgentMessageDelta<'a>),
    AgentMessageCompleted(StoredTranscriptAgentMessageCompleted<'a>),
    ReasoningSummaryDelta(StoredTranscriptReasoningSummaryDelta<'a>),
    ReasoningSummaryCompleted(StoredTranscriptReasoningSummaryCompleted<'a>),
    TerminalActivity(StoredTranscriptTerminalActivity<'a>),
    Tool(StoredTranscriptTool<'a>),
    File(StoredTranscriptFile<'a>),
    Search(StoredTranscriptSearch<'a>),
}

#[derive(Serialize)]
struct StoredTranscriptAgentMessageDelta<'a> {
    tag: &'static str,
    item_id: &'a str,
    phase: &'static str,
    delta: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptAgentMessageCompleted<'a> {
    tag: &'static str,
    item_id: &'a str,
    phase: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptReasoningSummaryDelta<'a> {
    tag: &'static str,
    item_id: &'a str,
    summary_index: u64,
    delta: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptReasoningSummaryCompleted<'a> {
    tag: &'static str,
    item_id: &'a str,
    text: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredTranscriptTerminalActivity<'a> {
    tag: &'static str,
    activity_id: &'a str,
    channel: Option<&'static str>,
    command: Option<&'a str>,
    exit_code: Option<i32>,
    output: Option<&'a str>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredTranscriptTool<'a> {
    tag: &'static str,
    tool_id: &'a str,
    tool_name: &'a str,
    action: &'static str,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredTranscriptFile<'a> {
    tag: &'static str,
    path: &'a str,
    action: &'static str,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

#[derive(Serialize)]
struct StoredTranscriptSearch<'a> {
    tag: &'static str,
    query: &'a str,
    result_count: Option<u64>,
    scope: Option<&'static str>,
    search_id: Option<&'a str>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredSubagentTranscript<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    agent_native_thread_id: &'a str,
    parent_native_thread_id: &'a str,
    content: StoredTranscriptContent<'a>,
}

#[derive(Serialize)]
struct StoredUsage<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    basis: &'static str,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    cost_usd: Option<f64>,
    provider_route_id: Option<&'a str>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredErrorRef<'a> {
    artisan_code: &'a str,
    provider_code: Option<&'a str>,
    detail: Option<&'a str>,
    affected_model_id: Option<&'a str>,
    limit_id: Option<&'a str>,
    limit_label: Option<&'a str>,
    limit_scope: Option<&'static str>,
    resets_at: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredNativeAction<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    action: &'a str,
    detail: Option<&'a str>,
    diagnostic: bool,
    error_ref: Option<StoredErrorRef<'a>>,
}

#[derive(Serialize)]
struct StoredProcessDiagnostic<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    level: &'static str,
    message: &'a str,
    error_ref: Option<StoredErrorRef<'a>>,
}

#[derive(Serialize)]
struct StoredProtocolDiagnostic<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    level: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredRunTerminal<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
    error_ref: Option<StoredErrorRef<'a>>,
    summary_title: Option<&'a str>,
}

fn stored_error_ref(value: &EngineErrorRef) -> StoredErrorRef<'_> {
    StoredErrorRef {
        artisan_code: value.artisan_code().as_str(),
        provider_code: value.provider_code(),
        detail: value.detail(),
        affected_model_id: value.affected_model_id(),
        limit_id: value.limit_id(),
        limit_label: value.limit_label(),
        limit_scope: value.limit_scope().map(LimitScope::as_str),
        resets_at: value.resets_at(),
    }
}

fn stored_transcript_content(value: &TranscriptContent) -> StoredTranscriptContent<'_> {
    match value {
        TranscriptContent::AgentMessageDelta(content) => {
            StoredTranscriptContent::AgentMessageDelta(StoredTranscriptAgentMessageDelta {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                phase: content.phase().as_str(),
                delta: content.delta(),
            })
        }
        TranscriptContent::AgentMessageCompleted(content) => {
            StoredTranscriptContent::AgentMessageCompleted(StoredTranscriptAgentMessageCompleted {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                phase: content.phase().as_str(),
                message: content.message(),
            })
        }
        TranscriptContent::ReasoningSummaryDelta(content) => {
            StoredTranscriptContent::ReasoningSummaryDelta(StoredTranscriptReasoningSummaryDelta {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                summary_index: content.summary_index(),
                delta: content.delta(),
            })
        }
        TranscriptContent::ReasoningSummaryCompleted(content) => {
            StoredTranscriptContent::ReasoningSummaryCompleted(
                StoredTranscriptReasoningSummaryCompleted {
                    tag: value.tag(),
                    item_id: content.item_id().as_str(),
                    text: content.text(),
                },
            )
        }
        TranscriptContent::TerminalActivity(content) => {
            StoredTranscriptContent::TerminalActivity(StoredTranscriptTerminalActivity {
                tag: value.tag(),
                activity_id: content.activity_id().as_str(),
                channel: content.channel().map(TerminalChannel::as_str),
                command: content.command(),
                exit_code: content.exit_code(),
                output: content.output(),
                state: content.state().as_str(),
            })
        }
        TranscriptContent::Tool(content) => StoredTranscriptContent::Tool(StoredTranscriptTool {
            tag: value.tag(),
            tool_id: content.tool_id().as_str(),
            tool_name: content.tool_name(),
            action: content.action().as_str(),
            detail: content.detail(),
        }),
        TranscriptContent::File(content) => StoredTranscriptContent::File(StoredTranscriptFile {
            tag: value.tag(),
            path: content.path(),
            action: content.action().as_str(),
            lines_added: content.lines_added(),
            lines_deleted: content.lines_deleted(),
        }),
        TranscriptContent::Search(content) => {
            StoredTranscriptContent::Search(StoredTranscriptSearch {
                tag: value.tag(),
                query: content.query(),
                result_count: content.result_count(),
                scope: content.scope().map(SearchScope::as_str),
                search_id: content.search_id().map(ObservationId::as_str),
                state: content.state().as_str(),
            })
        }
    }
}

#[allow(clippy::too_many_lines)]
fn stored_observation(observation: &Observation) -> StoredObservation<'_> {
    match observation {
        Observation::AgentMessageDelta(value) => {
            StoredObservation::AgentMessageDelta(StoredAgentMessageDelta {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                phase: value.phase().as_str(),
                delta: value.delta(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::AgentMessageCompleted(value) => {
            StoredObservation::AgentMessageCompleted(StoredAgentMessageCompleted {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                phase: value.phase().as_str(),
                message: value.message(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::Approval(value) => StoredObservation::Approval(StoredApproval {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            approval_id: value.approval_id().as_str(),
            state: value.state().as_str(),
            description: value.description(),
            request: StoredApprovalRequest {
                kind: value.request().kind().as_str(),
                command: value.request().command_text(),
                cwd: value.request().cwd(),
                reason: value.request().reason(),
            },
            approved: value.approved(),
        }),
        Observation::Compaction(value) => StoredObservation::Compaction(StoredCompaction {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
            compaction_id: value.compaction_id().map(ObservationId::as_str),
            duration_ms: value.duration_ms(),
            summary: value.summary(),
        }),
        Observation::File(value) => StoredObservation::File(StoredFile {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            path: value.path(),
            action: value.action().as_str(),
            lines_added: value.lines_added(),
            lines_deleted: value.lines_deleted(),
        }),
        Observation::NativeAction(value) => StoredObservation::NativeAction(StoredNativeAction {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            action: value.action(),
            detail: value.detail(),
            diagnostic: value.diagnostic(),
            error_ref: value.error_ref().map(stored_error_ref),
        }),
        Observation::Plan(value) => {
            let mut entries = Vec::with_capacity(value.entries().len());
            for entry in value.entries() {
                entries.push(StoredPlanEntry {
                    id: entry.id().as_str(),
                    status: entry.status().as_str(),
                    text: entry.text(),
                });
            }
            StoredObservation::Plan(StoredPlan {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                entries,
                turn_id: value.turn_id().map(ObservationId::as_str),
            })
        }
        Observation::ProcessDiagnostic(value) => {
            StoredObservation::ProcessDiagnostic(StoredProcessDiagnostic {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                level: value.level().as_str(),
                message: value.message(),
                error_ref: value.error_ref().map(stored_error_ref),
            })
        }
        Observation::ProtocolDiagnostic(value) => {
            StoredObservation::ProtocolDiagnostic(StoredProtocolDiagnostic {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                level: value.level().as_str(),
                message: value.message(),
            })
        }
        Observation::Question(value) => {
            let options = value.options().map(|options| {
                let mut stored = Vec::with_capacity(options.len());
                for option in options {
                    stored.push(StoredQuestionOption {
                        label: option.label(),
                        description: option.description(),
                    });
                }
                stored
            });
            StoredObservation::Question(StoredQuestion {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                question_id: value.question_id().as_str(),
                state: value.state().as_str(),
                text: value.text(),
                header: value.header(),
                multi_select: value.multi_select(),
                options,
                answers: value.answers(),
            })
        }
        Observation::ReasoningSummaryCompleted(value) => {
            StoredObservation::ReasoningSummaryCompleted(StoredReasoningSummaryCompleted {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                text: value.text(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::ReasoningSummaryDelta(value) => {
            StoredObservation::ReasoningSummaryDelta(StoredReasoningSummaryDelta {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                summary_index: value.summary_index(),
                delta: value.delta(),
                thinking_tokens: value.thinking_tokens(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::Retry(value) => StoredObservation::Retry(StoredRetry {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            turn_id: value.turn_id().as_str(),
            attempt_state: value.attempt_state().as_str(),
            will_retry: value.will_retry(),
            message: value.message(),
        }),
        Observation::RunState(value) => StoredObservation::RunState(StoredRunState {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
        }),
        Observation::RunTerminal(value) => StoredObservation::RunTerminal(StoredRunTerminal {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
            error_ref: value.error_ref().map(stored_error_ref),
            summary_title: value.summary_title(),
        }),
        Observation::Search(value) => StoredObservation::Search(StoredSearch {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            query: value.query(),
            scope: value.scope().map(SearchScope::as_str),
            search_id: value.search_id().map(ObservationId::as_str),
            state: value.state().as_str(),
            result_count: value.result_count(),
        }),
        Observation::Subagent(value) => StoredObservation::Subagent(StoredSubagent {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            agent_native_thread_id: value.agent_native_thread_id().as_str(),
            parent_native_thread_id: value.parent_native_thread_id().as_str(),
            state: value.state().as_str(),
            activity: value.activity(),
            agent_path: value.agent_path(),
            turn_id: value.turn_id().map(ObservationId::as_str),
        }),
        Observation::SubagentTranscript(value) => {
            StoredObservation::SubagentTranscript(StoredSubagentTranscript {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                agent_native_thread_id: value.agent_native_thread_id().as_str(),
                parent_native_thread_id: value.parent_native_thread_id().as_str(),
                content: stored_transcript_content(value.content()),
            })
        }
        Observation::TerminalActivity(value) => {
            StoredObservation::TerminalActivity(StoredTerminalActivity {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                activity_id: value.activity_id().as_str(),
                channel: value.channel().map(TerminalChannel::as_str),
                command: value.command(),
                shell: value.shell(),
                output: value.output(),
                exit_code: value.exit_code(),
                state: value.state().as_str(),
            })
        }
        Observation::Tool(value) => StoredObservation::Tool(StoredTool {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            tool_id: value.tool_id().as_str(),
            tool_name: value.tool_name(),
            action: value.action().as_str(),
            detail: value.detail(),
        }),
        Observation::TurnState(value) => StoredObservation::TurnState(StoredTurnState {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            turn_id: value.turn_id().as_str(),
            state: value.state().as_str(),
        }),
        Observation::Usage(value) => StoredObservation::Usage(StoredUsage {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            basis: value.basis().as_str(),
            input_tokens: value.input_tokens(),
            cached_input_tokens: value.cached_input_tokens(),
            output_tokens: value.output_tokens(),
            context_tokens: value.context_tokens(),
            context_window_tokens: value.context_window_tokens(),
            cost_usd: value.cost_usd(),
            provider_route_id: value.provider_route_id().map(ObservationId::as_str),
            turn_id: value.turn_id().map(ObservationId::as_str),
        }),
    }
}

// ---------------------------------------------------------------------------
// Strict decoding (exact key sets per tag, typed provider values)
// ---------------------------------------------------------------------------

fn require_keys(
    object: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), ObservationCommitError> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(ObservationCommitError::Malformed);
    }
    Ok(())
}

fn get_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_opt_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_bool(object: &Map<String, Value>, key: &str) -> Result<bool, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_i64(object: &Map<String, Value>, key: &str) -> Result<i64, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_u64(object: &Map<String, Value>, key: &str) -> Result<u64, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_opt_u64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_i32(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<i32>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_i64().ok_or(ObservationCommitError::Malformed)?;
            i32::try_from(raw)
                .map(Some)
                .map_err(|_| ObservationCommitError::Malformed)
        }
    }
}

fn get_opt_f64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_object()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_string_array(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_array().ok_or(ObservationCommitError::Malformed)?;
            let mut out = Vec::with_capacity(raw.len());
            for entry in raw {
                out.push(
                    entry
                        .as_str()
                        .ok_or(ObservationCommitError::Malformed)?
                        .to_owned(),
                );
            }
            Ok(Some(out))
        }
    }
}

fn get_opt_id(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<ObservationId>, ObservationCommitError> {
    match get_opt_str(object, key)? {
        None => Ok(None),
        Some(text) => ObservationId::parse(text.to_owned())
            .map(Some)
            .map_err(ObservationError::Identifier)
            .map_err(ObservationCommitError::InvalidObservation),
    }
}

fn header(
    object: &Map<String, Value>,
) -> Result<(ObservationId, ObservationSequence), ObservationCommitError> {
    let id = ObservationId::parse(get_str(object, "id")?.to_owned())
        .map_err(ObservationError::Identifier)?;
    let sequence = ObservationSequence::new(get_u64(object, "sequence")?)?;
    Ok((id, sequence))
}

fn decode_error_ref(object: &Map<String, Value>) -> Result<EngineErrorRef, ObservationCommitError> {
    require_keys(
        object,
        &[
            "artisan_code",
            "provider_code",
            "detail",
            "affected_model_id",
            "limit_id",
            "limit_label",
            "limit_scope",
            "resets_at",
        ],
    )?;
    let artisan_code = ArtisanCode::parse(get_str(object, "artisan_code")?.to_owned())?;
    let limit_scope = match get_opt_str(object, "limit_scope")? {
        None => None,
        Some(scope) => Some(LimitScope::parse(scope)?),
    };
    EngineErrorRef::new(EngineErrorRefInput {
        artisan_code,
        provider_code: get_opt_str(object, "provider_code")?.map(str::to_owned),
        detail: get_opt_str(object, "detail")?.map(str::to_owned),
        affected_model_id: get_opt_str(object, "affected_model_id")?.map(str::to_owned),
        limit_id: get_opt_str(object, "limit_id")?.map(str::to_owned),
        limit_label: get_opt_str(object, "limit_label")?.map(str::to_owned),
        limit_scope,
        resets_at: get_opt_str(object, "resets_at")?.map(str::to_owned),
    })
    .map_err(ObservationCommitError::InvalidObservation)
}

fn decode_opt_error_ref(
    object: &Map<String, Value>,
) -> Result<Option<EngineErrorRef>, ObservationCommitError> {
    match get_opt_object(object, "error_ref")? {
        None => Ok(None),
        Some(nested) => decode_error_ref(nested).map(Some),
    }
}

fn decode_approval_request(
    object: &Map<String, Value>,
) -> Result<ApprovalRequest, ObservationCommitError> {
    require_keys(object, &["kind", "command", "cwd", "reason"])?;
    let kind = ApprovalKind::parse(get_str(object, "kind")?)?;
    match kind {
        ApprovalKind::Command => ApprovalRequest::command(
            get_str(object, "command")?.to_owned(),
            get_opt_str(object, "cwd")?.map(str::to_owned),
            get_opt_str(object, "reason")?.map(str::to_owned),
        ),
        ApprovalKind::FileChange => {
            if get_opt_str(object, "command")?.is_some() || get_opt_str(object, "cwd")?.is_some() {
                return Err(ObservationCommitError::Malformed);
            }
            ApprovalRequest::file_change(get_opt_str(object, "reason")?.map(str::to_owned))
        }
        ApprovalKind::Action => {
            if get_opt_str(object, "command")?.is_some() || get_opt_str(object, "cwd")?.is_some() {
                return Err(ObservationCommitError::Malformed);
            }
            ApprovalRequest::action(get_opt_str(object, "reason")?.map(str::to_owned))
        }
    }
    .map_err(ObservationCommitError::InvalidObservation)
}

fn decode_question_options(
    object: &Map<String, Value>,
) -> Result<Option<Vec<QuestionOption>>, ObservationCommitError> {
    match object.get("options") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_array().ok_or(ObservationCommitError::Malformed)?;
            let mut options = Vec::with_capacity(raw.len());
            for entry in raw {
                let option = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
                require_keys(option, &["label", "description"])?;
                options.push(
                    QuestionOption::new(
                        get_str(option, "label")?.to_owned(),
                        get_opt_str(option, "description")?.map(str::to_owned),
                    )
                    .map_err(ObservationCommitError::InvalidObservation)?,
                );
            }
            Ok(Some(options))
        }
    }
}

fn decode_plan_entries(
    object: &Map<String, Value>,
) -> Result<Vec<PlanEntry>, ObservationCommitError> {
    let raw = object
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(ObservationCommitError::Malformed)?;
    let mut entries = Vec::with_capacity(raw.len());
    for entry in raw {
        let item = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
        require_keys(item, &["id", "status", "text"])?;
        entries.push(
            PlanEntry::new(
                ObservationId::parse(get_str(item, "id")?.to_owned())
                    .map_err(ObservationError::Identifier)
                    .map_err(ObservationCommitError::InvalidObservation)?,
                PlanEntryStatus::parse(get_str(item, "status")?)
                    .map_err(ObservationCommitError::InvalidObservation)?,
                get_str(item, "text")?.to_owned(),
            )
            .map_err(ObservationCommitError::InvalidObservation)?,
        );
    }
    Ok(entries)
}

fn decode_transcript_content(
    object: &Map<String, Value>,
) -> Result<TranscriptContent, ObservationCommitError> {
    let tag = get_str(object, "tag")?;
    match tag {
        "agent_message_delta" => {
            require_keys(object, &["tag", "item_id", "phase", "delta"])?;
            TranscriptAgentMessageDelta::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "delta")?.to_owned(),
            )
            .map(TranscriptContent::AgentMessageDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "agent_message_completed" => {
            require_keys(object, &["tag", "item_id", "phase", "message"])?;
            TranscriptAgentMessageCompleted::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "message")?.to_owned(),
            )
            .map(TranscriptContent::AgentMessageCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_delta" => {
            require_keys(object, &["tag", "item_id", "summary_index", "delta"])?;
            TranscriptReasoningSummaryDelta::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_u64(object, "summary_index")?,
                get_str(object, "delta")?.to_owned(),
            )
            .map(TranscriptContent::ReasoningSummaryDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_completed" => {
            require_keys(object, &["tag", "item_id", "text"])?;
            TranscriptReasoningSummaryCompleted::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_opt_str(object, "text")?.map(str::to_owned),
            )
            .map(TranscriptContent::ReasoningSummaryCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "terminal_activity" => {
            require_keys(
                object,
                &[
                    "tag",
                    "activity_id",
                    "channel",
                    "command",
                    "exit_code",
                    "output",
                    "state",
                ],
            )?;
            let channel = match get_opt_str(object, "channel")? {
                None => None,
                Some(channel) => Some(TerminalChannel::parse(channel)?),
            };
            TranscriptTerminalActivity::new(
                ObservationId::parse(get_str(object, "activity_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                channel,
                get_opt_str(object, "command")?.map(str::to_owned),
                get_opt_i32(object, "exit_code")?,
                get_opt_str(object, "output")?.map(str::to_owned),
                TerminalActivityState::parse(get_str(object, "state")?)?,
            )
            .map(TranscriptContent::TerminalActivity)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "tool" => {
            require_keys(object, &["tag", "tool_id", "tool_name", "action", "detail"])?;
            TranscriptTool::new(
                ObservationId::parse(get_str(object, "tool_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_str(object, "tool_name")?.to_owned(),
                ToolAction::parse(get_str(object, "action")?)?,
                get_opt_str(object, "detail")?.map(str::to_owned),
            )
            .map(TranscriptContent::Tool)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "file" => {
            require_keys(
                object,
                &["tag", "path", "action", "lines_added", "lines_deleted"],
            )?;
            TranscriptFile::new(
                get_str(object, "path")?.to_owned(),
                FileAction::parse(get_str(object, "action")?)?,
                get_opt_u64(object, "lines_added")?,
                get_opt_u64(object, "lines_deleted")?,
            )
            .map(TranscriptContent::File)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "search" => {
            require_keys(
                object,
                &[
                    "tag",
                    "query",
                    "result_count",
                    "scope",
                    "search_id",
                    "state",
                ],
            )?;
            let scope = match get_opt_str(object, "scope")? {
                None => None,
                Some(scope) => Some(SearchScope::parse(scope)?),
            };
            TranscriptSearch::new(
                get_str(object, "query")?.to_owned(),
                get_opt_u64(object, "result_count")?,
                scope,
                get_opt_id(object, "search_id")?,
                SearchState::parse(get_str(object, "state")?)?,
            )
            .map(TranscriptContent::Search)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        _ => Err(ObservationCommitError::UnknownObservation),
    }
}

#[allow(clippy::too_many_lines)]
fn decode_observation(object: &Map<String, Value>) -> Result<Observation, ObservationCommitError> {
    let tag = get_str(object, "tag")?;
    match tag {
        "agent_message_delta" => {
            require_keys(
                object,
                &[
                    "tag", "id", "sequence", "item_id", "phase", "delta", "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            AgentMessageDeltaObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "delta")?.to_owned(),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::AgentMessageDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "agent_message_completed" => {
            require_keys(
                object,
                &[
                    "tag", "id", "sequence", "item_id", "phase", "message", "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            AgentMessageCompletedObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "message")?.to_owned(),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::AgentMessageCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "approval" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "approval_id",
                    "state",
                    "description",
                    "request",
                    "approved",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let approval_id = ObservationId::parse(get_str(object, "approval_id")?.to_owned())
                .map_err(ObservationError::Identifier)?;
            let state = ApprovalState::parse(get_str(object, "state")?)?;
            let description = get_str(object, "description")?.to_owned();
            let request_object = object
                .get("request")
                .and_then(Value::as_object)
                .ok_or(ObservationCommitError::Malformed)?;
            let request = decode_approval_request(request_object)?;
            let approved = match object.get("approved") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_bool().ok_or(ObservationCommitError::Malformed)?),
            };
            match (state, approved) {
                (ApprovalState::Requested, None) => {
                    ApprovalObservation::requested(id, sequence, approval_id, description, request)
                }
                (ApprovalState::Resolved, Some(decision)) => ApprovalObservation::resolved(
                    id,
                    sequence,
                    approval_id,
                    description,
                    request,
                    decision,
                ),
                (ApprovalState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "approved" })
                }
                (ApprovalState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "approved" })
                }
            }
            .map(Observation::Approval)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "compaction" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "state",
                    "compaction_id",
                    "duration_ms",
                    "summary",
                ],
            )?;
            let (id, sequence) = header(object)?;
            CompactionObservation::new(
                id,
                sequence,
                CompactionState::parse(get_str(object, "state")?)?,
                get_opt_id(object, "compaction_id")?,
                get_opt_u64(object, "duration_ms")?,
                get_opt_str(object, "summary")?.map(str::to_owned),
            )
            .map(Observation::Compaction)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "file" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "path",
                    "action",
                    "lines_added",
                    "lines_deleted",
                ],
            )?;
            let (id, sequence) = header(object)?;
            FileObservation::new(
                id,
                sequence,
                get_str(object, "path")?.to_owned(),
                FileAction::parse(get_str(object, "action")?)?,
                get_opt_u64(object, "lines_added")?,
                get_opt_u64(object, "lines_deleted")?,
            )
            .map(Observation::File)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "native_action" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "action",
                    "detail",
                    "diagnostic",
                    "error_ref",
                ],
            )?;
            let (id, sequence) = header(object)?;
            NativeActionObservation::new(
                id,
                sequence,
                get_str(object, "action")?.to_owned(),
                get_opt_str(object, "detail")?.map(str::to_owned),
                get_bool(object, "diagnostic")?,
                decode_opt_error_ref(object)?,
            )
            .map(Observation::NativeAction)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "plan" => {
            require_keys(object, &["tag", "id", "sequence", "entries", "turn_id"])?;
            let (id, sequence) = header(object)?;
            PlanObservation::new(
                id,
                sequence,
                decode_plan_entries(object)?,
                get_opt_id(object, "turn_id")?,
            )
            .map(Observation::Plan)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "process_diagnostic" => {
            require_keys(
                object,
                &["tag", "id", "sequence", "level", "message", "error_ref"],
            )?;
            let (id, sequence) = header(object)?;
            ProcessDiagnosticObservation::new(
                id,
                sequence,
                DiagnosticLevel::parse(get_str(object, "level")?)?,
                get_str(object, "message")?.to_owned(),
                decode_opt_error_ref(object)?,
            )
            .map(Observation::ProcessDiagnostic)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "protocol_diagnostic" => {
            require_keys(object, &["tag", "id", "sequence", "level", "message"])?;
            let (id, sequence) = header(object)?;
            ProtocolDiagnosticObservation::new(
                id,
                sequence,
                DiagnosticLevel::parse(get_str(object, "level")?)?,
                get_str(object, "message")?.to_owned(),
            )
            .map(Observation::ProtocolDiagnostic)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "question" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "question_id",
                    "state",
                    "text",
                    "header",
                    "multi_select",
                    "options",
                    "answers",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let input = QuestionInput {
                question_id: ObservationId::parse(get_str(object, "question_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                text: get_str(object, "text")?.to_owned(),
                header: get_opt_str(object, "header")?.map(str::to_owned),
                multi_select: get_bool(object, "multi_select")?,
                options: decode_question_options(object)?,
            };
            let state = QuestionState::parse(get_str(object, "state")?)?;
            let answers = get_opt_string_array(object, "answers")?;
            match (state, answers) {
                (QuestionState::Requested, None) => {
                    QuestionObservation::requested(id, sequence, input)
                }
                (QuestionState::Resolved, Some(resolved)) => {
                    QuestionObservation::resolved(id, sequence, input, resolved)
                }
                (QuestionState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "answers" })
                }
                (QuestionState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "answers" })
                }
            }
            .map(Observation::Question)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_completed" => {
            require_keys(
                object,
                &["tag", "id", "sequence", "item_id", "text", "turn_id"],
            )?;
            let (id, sequence) = header(object)?;
            ReasoningSummaryCompletedObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_opt_str(object, "text")?.map(str::to_owned),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::ReasoningSummaryCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_delta" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "item_id",
                    "summary_index",
                    "delta",
                    "thinking_tokens",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            ReasoningSummaryDeltaObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_u64(object, "summary_index")?,
                get_str(object, "delta")?.to_owned(),
                get_opt_u64(object, "thinking_tokens")?,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::ReasoningSummaryDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "retry" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "turn_id",
                    "attempt_state",
                    "will_retry",
                    "message",
                ],
            )?;
            let (id, sequence) = header(object)?;
            RetryObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                RetryAttemptState::parse(get_str(object, "attempt_state")?)?,
                get_bool(object, "will_retry")?,
                get_str(object, "message")?.to_owned(),
            )
            .map(Observation::Retry)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "run_state" => {
            require_keys(object, &["tag", "id", "sequence", "state"])?;
            let (id, sequence) = header(object)?;
            Ok(Observation::RunState(RunStateObservation::new(
                id,
                sequence,
                RunState::parse(get_str(object, "state")?)?,
            )))
        }
        "run_terminal" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "state",
                    "error_ref",
                    "summary_title",
                ],
            )?;
            let (id, sequence) = header(object)?;
            RunTerminalObservation::new(
                id,
                sequence,
                RunTerminalState::parse(get_str(object, "state")?)?,
                decode_opt_error_ref(object)?,
                get_opt_str(object, "summary_title")?.map(str::to_owned),
            )
            .map(Observation::RunTerminal)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "search" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "query",
                    "scope",
                    "search_id",
                    "state",
                    "result_count",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let scope = match get_opt_str(object, "scope")? {
                None => None,
                Some(scope) => Some(SearchScope::parse(scope)?),
            };
            SearchObservation::new(
                id,
                sequence,
                get_str(object, "query")?.to_owned(),
                scope,
                get_opt_id(object, "search_id")?,
                SearchState::parse(get_str(object, "state")?)?,
                get_opt_u64(object, "result_count")?,
            )
            .map(Observation::Search)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "subagent" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "agent_native_thread_id",
                    "parent_native_thread_id",
                    "state",
                    "activity",
                    "agent_path",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            SubagentObservation::new(
                id,
                sequence,
                SubagentInput {
                    agent_native_thread_id: ObservationId::parse(
                        get_str(object, "agent_native_thread_id")?.to_owned(),
                    )
                    .map_err(ObservationError::Identifier)?,
                    parent_native_thread_id: ObservationId::parse(
                        get_str(object, "parent_native_thread_id")?.to_owned(),
                    )
                    .map_err(ObservationError::Identifier)?,
                    state: SubagentState::parse(get_str(object, "state")?)?,
                    activity: get_opt_str(object, "activity")?.map(str::to_owned),
                    agent_path: get_opt_str(object, "agent_path")?.map(str::to_owned),
                    turn_id: get_opt_id(object, "turn_id")?,
                },
            )
            .map(Observation::Subagent)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "subagent_transcript" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "agent_native_thread_id",
                    "parent_native_thread_id",
                    "content",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let content_object = object
                .get("content")
                .and_then(Value::as_object)
                .ok_or(ObservationCommitError::Malformed)?;
            Ok(Observation::SubagentTranscript(
                SubagentTranscriptObservation::new(
                    id,
                    sequence,
                    ObservationId::parse(get_str(object, "agent_native_thread_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    ObservationId::parse(get_str(object, "parent_native_thread_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    decode_transcript_content(content_object)?,
                ),
            ))
        }
        "terminal_activity" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "activity_id",
                    "channel",
                    "command",
                    "shell",
                    "output",
                    "exit_code",
                    "state",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let channel = match get_opt_str(object, "channel")? {
                None => None,
                Some(channel) => Some(TerminalChannel::parse(channel)?),
            };
            TerminalActivityObservation::new(
                id,
                sequence,
                TerminalActivityInput {
                    activity_id: ObservationId::parse(get_str(object, "activity_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    channel,
                    command: get_opt_str(object, "command")?.map(str::to_owned),
                    shell: get_opt_str(object, "shell")?.map(str::to_owned),
                    output: get_opt_str(object, "output")?.map(str::to_owned),
                    exit_code: get_opt_i32(object, "exit_code")?,
                    state: TerminalActivityState::parse(get_str(object, "state")?)?,
                },
            )
            .map(Observation::TerminalActivity)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "tool" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "tool_id",
                    "tool_name",
                    "action",
                    "detail",
                ],
            )?;
            let (id, sequence) = header(object)?;
            ToolObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "tool_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_str(object, "tool_name")?.to_owned(),
                ToolAction::parse(get_str(object, "action")?)?,
                get_opt_str(object, "detail")?.map(str::to_owned),
            )
            .map(Observation::Tool)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "turn_state" => {
            require_keys(object, &["tag", "id", "sequence", "turn_id", "state"])?;
            let (id, sequence) = header(object)?;
            Ok(Observation::TurnState(TurnStateObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                TurnState::parse(get_str(object, "state")?)?,
            )))
        }
        "usage" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "basis",
                    "input_tokens",
                    "cached_input_tokens",
                    "output_tokens",
                    "context_tokens",
                    "context_window_tokens",
                    "cost_usd",
                    "provider_route_id",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            UsageObservation::new(
                id,
                sequence,
                UsageInput {
                    basis: UsageBasis::parse(get_str(object, "basis")?)?,
                    input_tokens: get_opt_u64(object, "input_tokens")?,
                    cached_input_tokens: get_opt_u64(object, "cached_input_tokens")?,
                    output_tokens: get_opt_u64(object, "output_tokens")?,
                    context_tokens: get_opt_u64(object, "context_tokens")?,
                    context_window_tokens: get_opt_u64(object, "context_window_tokens")?,
                    cost_usd: get_opt_f64(object, "cost_usd")?,
                    provider_route_id: get_opt_id(object, "provider_route_id")?,
                    turn_id: get_opt_id(object, "turn_id")?,
                },
            )
            .map(Observation::Usage)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        _ => Err(ObservationCommitError::UnknownObservation),
    }
}
