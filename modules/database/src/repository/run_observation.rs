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
mod codec;
mod commit;
mod projection;
pub mod terminal;

use artisan_domain::{
    AssistantBody, AssistantMessagePhase, IncrementalText, ItemId, MessageId, PatchId, Revision,
    RunId, UnixMillis,
};
use thiserror::Error;
use zeroize::Zeroize;

use crate::repository::message_dispatch::ClaimedMessageDispatch;
use crate::repository::run_binding::BoundRunReceipt;
use crate::repository::run_launch::{LaunchedRunReceipt, RunLaunchCredentials, RunStartKey};

use super::RepositoryError;

pub use self::codec::{
    DecodedObservationBatch, OBSERVATION_BATCH_MAX_OBSERVATIONS, OBSERVATION_CHECKPOINT_VERSION,
    OBSERVATION_FORMAT_TAG, ObservationCommitError, decode_observation_checkpoint,
    encode_observation_bytes, encode_observation_checkpoint, validate_observation_bind,
    validate_observation_engine,
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
