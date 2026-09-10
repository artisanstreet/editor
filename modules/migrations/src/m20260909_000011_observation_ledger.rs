//! Durable append-only engine-observation ledger.
//!
//! The existing `run_checkpoints` row keeps only the latest batch per run
//! (`CheckpointUpdate::Replace` overwrites the blob), so earlier batches are
//! durably unreadable once a second batch commits. This migration adds the
//! `observation_ledger` table that preserves every committed observation:
//! one immutable row per observation, appended atomically inside the same
//! `commit_run_batch` transaction that persists the checkpoint and receipt.
//!
//! Keying:
//!
//! - `(thread_id, delivery_sequence)` is the primary key. The delivery
//!   sequence is a thread-scoped strictly increasing positive counter
//!   allocated in the commit transaction, separate from the run-local
//!   observation sequence the codec owns. Subscription cursors advance on it.
//! - `(run_id, observation_sequence)` is unique, so a run-local sequence is
//!   never reused within its run while two runs on one thread may each start
//!   at sequence one.
//!
//! Each row pins the Forge turn the producing run launched from (resolved
//! from the run scope at commit time, never from a provider string), the
//! caller-injected commit instant (no new wall-clock), and the canonical
//! single-observation envelope produced by the existing batch codec.
//!
//! The run, thread, and Forge-turn identities are bound with `RESTRICT`
//! foreign keys following the native `run_checkpoints` pattern: ledger rows
//! are immutable history, so a referenced run, thread, or turn can neither
//! be removed nor re-keyed while history names it. No existing code path
//! deletes those parents, so the keys never block a live transition.

use sea_orm_migration::prelude::*;

const IDENTIFIER_MAX_BYTES: i64 = 128;
const ENGINE_TAG_MAX_BYTES: i64 = 64;
const OBSERVATION_BYTES_MAX: i64 = 262_144;
const SEQUENCE_MAX: i64 = 9_223_372_036_854_775_807;

/// Creates the append-only observation ledger for committed engine observations.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE observation_ledger (\
                    thread_id TEXT NOT NULL,\
                    delivery_sequence INTEGER NOT NULL,\
                    run_id TEXT NOT NULL,\
                    observation_sequence INTEGER NOT NULL,\
                    turn_id TEXT NOT NULL,\
                    committed_at_ms INTEGER NOT NULL,\
                    engine TEXT NOT NULL,\
                    binding_version INTEGER NOT NULL,\
                    observation_version INTEGER NOT NULL,\
                    observation_bytes BLOB NOT NULL,\
                    PRIMARY KEY (thread_id, delivery_sequence),\
                    UNIQUE (run_id, observation_sequence),\
                    FOREIGN KEY (run_id) REFERENCES assistant_runs(run_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (turn_id) REFERENCES conversation_turns(turn_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(thread_id) = 'text' AND length(CAST(thread_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(delivery_sequence) = 'integer' AND delivery_sequence BETWEEN 1 AND {SEQUENCE_MAX}),\
                    CHECK (typeof(run_id) = 'text' AND length(CAST(run_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(observation_sequence) = 'integer' AND observation_sequence BETWEEN 1 AND {SEQUENCE_MAX}),\
                    CHECK (typeof(turn_id) = 'text' AND length(CAST(turn_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(committed_at_ms) = 'integer'),\
                    CHECK (typeof(engine) = 'text' AND length(CAST(engine AS BLOB)) BETWEEN 1 AND {ENGINE_TAG_MAX_BYTES}),\
                    CHECK (typeof(binding_version) = 'integer' AND binding_version BETWEEN 1 AND {SEQUENCE_MAX}),\
                    CHECK (typeof(observation_version) = 'integer' AND observation_version BETWEEN 1 AND {SEQUENCE_MAX}),\
                    CHECK (typeof(observation_bytes) = 'blob' AND length(observation_bytes) BETWEEN 1 AND {OBSERVATION_BYTES_MAX})\
                )"
            ))
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP TABLE IF EXISTS observation_ledger")
            .await
            .map(|_| ())
    }
}
