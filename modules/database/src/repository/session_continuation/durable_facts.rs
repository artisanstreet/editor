//! Durable checkpoint and sequence facts of one continuation candidate.

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};

use artisan_domain::{RunId, UnixMillis};

use crate::entities;
use crate::repository::{RepositoryError, corrupt_data, database_error};

use super::{SessionContinuationCheckpoint, SessionContinuationSequence};

#[expect(
    clippy::too_many_lines,
    reason = "keeps the four dependent reads of one continuation snapshot in a single linear \
              sequence; extraction would split the transaction locals"
)]
pub(super) async fn read_durable_facts<C: ConnectionTrait>(
    database: &C,
    run: &entities::assistant_run::Model,
    run_id: &RunId,
) -> Result<
    (
        Option<SessionContinuationCheckpoint>,
        SessionContinuationSequence,
    ),
    RepositoryError,
> {
    let checkpoint = entities::run_checkpoint::Entity::find_by_id(run_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read continuation checkpoint", source))?;
    let receipt = entities::run_batch_receipt::Entity::find()
        .filter(entities::run_batch_receipt::Column::RunId.eq(run_id.as_str()))
        .order_by_desc(entities::run_batch_receipt::Column::BatchSequence)
        .one(database)
        .await
        .map_err(|source| database_error("read continuation batch receipt", source))?;
    let state = entities::conversation_state::Entity::find_by_id(run.thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read continuation thread sequence", source))?;

    if let Some(state) = &state
        && state.last_patch_sequence < 0
    {
        return Err(corrupt_data(
            "conversation_state",
            "last_patch_sequence",
            "conversation patch sequence is negative",
        ));
    }
    if let Some(receipt) = &receipt {
        if !receipt.committed {
            return Err(corrupt_data(
                "run_batch_receipts",
                "committed",
                "uncommitted receipt is not a durable sequence fact",
            ));
        }
        if receipt.generation != run.generation || receipt.batch_sequence <= 0 {
            return Err(corrupt_data(
                "run_batch_receipts",
                "generation",
                "receipt generation or sequence is incompatible with its run",
            ));
        }
    }

    let checkpoint_facts = if let Some(row) = checkpoint {
        if row.generation != run.generation
            || row.last_batch_sequence < 0
            || row.updated_at_ms < run.created_at_ms
            || row.updated_at_ms > run.updated_at_ms
        {
            return Err(corrupt_data(
                "run_checkpoints",
                "generation",
                "checkpoint facts are outside the run fence",
            ));
        }
        let has_version = row.engine_checkpoint_version.is_some();
        let has_blob = row.engine_checkpoint_blob.is_some();
        if has_version != has_blob
            || row
                .engine_checkpoint_version
                .is_some_and(|version| version <= 0)
            || row
                .engine_checkpoint_blob
                .as_ref()
                .is_some_and(|blob| blob.as_slice().is_empty() || blob.as_slice().len() > 262_144)
        {
            return Err(corrupt_data(
                "run_checkpoints",
                "engine_checkpoint_blob",
                "checkpoint payload tuple is invalid",
            ));
        }
        if let Some(receipt) = &receipt {
            if row.last_batch_sequence != receipt.batch_sequence {
                return Err(corrupt_data(
                    "run_checkpoints",
                    "last_batch_sequence",
                    "checkpoint and receipt sequences disagree",
                ));
            }
        } else if row.last_batch_sequence != 0 {
            return Err(corrupt_data(
                "run_checkpoints",
                "last_batch_sequence",
                "checkpoint has no corresponding batch receipt",
            ));
        }
        Some(SessionContinuationCheckpoint {
            generation: row.generation,
            last_batch_sequence: row.last_batch_sequence,
            engine_checkpoint_version: row.engine_checkpoint_version,
            has_engine_checkpoint: has_blob,
            updated_at: UnixMillis::from_millis(row.updated_at_ms),
        })
    } else {
        if receipt.is_some() {
            return Err(corrupt_data(
                "run_checkpoints",
                "run_id",
                "batch receipt exists without a checkpoint row",
            ));
        }
        None
    };

    let last_batch_sequence = checkpoint_facts
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.last_batch_sequence);
    Ok((
        checkpoint_facts,
        SessionContinuationSequence {
            last_batch_sequence,
            last_committed_batch_sequence: receipt.map(|row| row.batch_sequence),
            last_patch_sequence: state.map(|row| row.last_patch_sequence),
        },
    ))
}
