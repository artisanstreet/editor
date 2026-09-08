//! Durable pending approval/question rows plus idempotent response receipts.
//!
//! The native stack previously had approval *presentation* only: no table
//! recorded that a provider asked, and no receipt correlated a client answer.
//! This migration adds both, keyed exactly as the A-approve contract
//! requires:
//!
//! - `pending_run_interactions` rows are keyed by `(run_id, interaction_id)`
//!   and carry the requested/resolved lifecycle with its resolution. Rows are
//!   per-run and deleted when the run settles, so decisions never leak across
//!   runs. Each row pins the provider binding version observed when the
//!   request was recorded, so a response for a rebound run is rejected
//!   instead of misapplied.
//! - `run_interaction_receipts` rows are keyed by the client-minted request
//!   id and carry the exact intent fingerprint plus the settled outcome, so
//!   replays answer `duplicate` with no second effect while a reused id with
//!   a different intent is a conflict, never a silent second resolution.
//!
//! Request snapshots ride as canonical JSON validated by the repository
//! through the domain constructors on both write and read; the schema only
//! enforces shape coherence (state/resolution agreement) and byte bounds.

use sea_orm_migration::prelude::*;

const IDENTIFIER_MAX_BYTES: i64 = 128;
const INTENT_KEY_MAX_BYTES: i64 = 32_768;
const REQUEST_JSON_MAX_BYTES: i64 = 32_768;
const ANSWERS_JSON_MAX_BYTES: i64 = 32_768;

/// Creates the pending-request and response-receipt tables for A-approve.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE pending_run_interactions (\
                    interaction_pk INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,\
                    run_id TEXT NOT NULL,\
                    interaction_id TEXT NOT NULL,\
                    thread_id TEXT NOT NULL,\
                    kind TEXT NOT NULL,\
                    state TEXT NOT NULL,\
                    request_json TEXT NOT NULL,\
                    requested_sequence INTEGER NOT NULL,\
                    approved INTEGER NULL,\
                    answers_json TEXT NULL,\
                    requested_at_ms INTEGER NOT NULL,\
                    resolved_at_ms INTEGER NULL,\
                    resolved_sequence INTEGER NULL,\
                    binding_version INTEGER NOT NULL,\
                    UNIQUE (run_id, interaction_id),\
                    CHECK (typeof(run_id) = 'text' AND length(CAST(run_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(interaction_id) = 'text' AND length(CAST(interaction_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(thread_id) = 'text' AND length(CAST(thread_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (kind IN ('approval', 'question')),\
                    CHECK (state IN ('requested', 'resolved')),\
                    CHECK (typeof(request_json) = 'text' AND length(CAST(request_json AS BLOB)) BETWEEN 2 AND {REQUEST_JSON_MAX_BYTES}),\
                    CHECK (typeof(requested_sequence) = 'integer' AND requested_sequence BETWEEN 1 AND 9223372036854775807),\
                    CHECK (typeof(requested_at_ms) = 'integer'),\
                    CHECK (typeof(binding_version) = 'integer' AND binding_version BETWEEN 1 AND 9223372036854775807),\
                    CHECK ((state = 'requested' AND resolved_at_ms IS NULL AND resolved_sequence IS NULL AND approved IS NULL AND answers_json IS NULL) OR \
                           (state = 'resolved' AND typeof(resolved_at_ms) = 'integer' AND typeof(resolved_sequence) = 'integer' AND resolved_sequence BETWEEN 1 AND 9223372036854775807 AND \
                            ((kind = 'approval' AND approved IN (0, 1) AND answers_json IS NULL) OR \
                             (kind = 'question' AND approved IS NULL AND typeof(answers_json) = 'text' AND length(CAST(answers_json AS BLOB)) BETWEEN 2 AND {ANSWERS_JSON_MAX_BYTES}))))\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_pending_run_interactions_run ON pending_run_interactions(run_id)",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_pending_run_interactions_open ON pending_run_interactions(state, requested_at_ms)",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE run_interaction_receipts (\
                    request_id TEXT NOT NULL PRIMARY KEY,\
                    command_kind TEXT NOT NULL,\
                    thread_id TEXT NOT NULL,\
                    run_id TEXT NOT NULL,\
                    interaction_id TEXT NOT NULL,\
                    outcome TEXT NOT NULL,\
                    disposition TEXT NOT NULL,\
                    intent_key TEXT NOT NULL,\
                    approved INTEGER NULL,\
                    answers_json TEXT NULL,\
                    binding_version INTEGER NOT NULL,\
                    responded_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(request_id) = 'text' AND length(CAST(request_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (command_kind IN ('respond_approval', 'respond_question')),\
                    CHECK (typeof(thread_id) = 'text' AND length(CAST(thread_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(run_id) = 'text' AND length(CAST(run_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(interaction_id) = 'text' AND length(CAST(interaction_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (outcome IN ('applied', 'unknown_target', 'already_resolved', 'wrong_run')),\
                    CHECK (disposition IN ('accepted', 'duplicate')),\
                    CHECK (typeof(intent_key) = 'text' AND length(CAST(intent_key AS BLOB)) BETWEEN 1 AND {INTENT_KEY_MAX_BYTES}),\
                    CHECK (typeof(binding_version) = 'integer' AND binding_version BETWEEN 1 AND 9223372036854775807),\
                    CHECK (typeof(responded_at_ms) = 'integer'),\
                    CHECK (((command_kind = 'respond_approval' AND approved IN (0, 1) AND answers_json IS NULL) OR \
                            (command_kind = 'respond_question' AND approved IS NULL AND typeof(answers_json) = 'text' AND length(CAST(answers_json AS BLOB)) BETWEEN 2 AND {ANSWERS_JSON_MAX_BYTES})))\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_run_interaction_receipts_run_target ON run_interaction_receipts(run_id, interaction_id)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_run_interaction_receipts_run_target")
            .await?;
        connection
            .execute_unprepared("DROP TABLE IF EXISTS run_interaction_receipts")
            .await?;
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_pending_run_interactions_open")
            .await?;
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_pending_run_interactions_run")
            .await?;
        connection
            .execute_unprepared("DROP TABLE IF EXISTS pending_run_interactions")
            .await
            .map(|_| ())
    }
}
