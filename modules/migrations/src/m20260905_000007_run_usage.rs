//! Adds the bounded current provider-usage record for one assistant run.
//!
//! The table is deliberately one row per durable run. Repository code owns
//! the provider-session and monotonic-sequence rules; SQLite owns the bounded
//! scalar shape and the same-thread run foreign key. No provider response,
//! prompt, credential, or raw event payload is stored.

use sea_orm_migration::prelude::*;

const RUN_USAGE_TABLE: &str = r#"
CREATE TABLE run_usage (
    run_id TEXT NOT NULL PRIMARY KEY,
    thread_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    provider_session_id TEXT NOT NULL,
    source_sequence INTEGER NOT NULL,
    basis TEXT NOT NULL,
    provider_turn_id TEXT NULL,
    model_id TEXT NOT NULL,
    provider_route_id TEXT NOT NULL,
    variant_id TEXT NULL,
    input_tokens INTEGER NULL,
    cached_input_tokens INTEGER NULL,
    output_tokens INTEGER NULL,
    context_tokens INTEGER NULL,
    context_window_tokens INTEGER NULL,
    observed_at_ms INTEGER NOT NULL,
    FOREIGN KEY (run_id, thread_id)
        REFERENCES assistant_runs (run_id, thread_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    CHECK (typeof(generation) = 'integer' AND generation >= 0),
    CHECK (typeof(provider_session_id) = 'text'
        AND length(CAST(provider_session_id AS BLOB)) BETWEEN 1 AND 128),
    CHECK (typeof(source_sequence) = 'integer'
        AND source_sequence BETWEEN 0 AND 9223372036854775807),
    CHECK (basis IN ('delta', 'cumulative', 'unknown')),
    CHECK (provider_turn_id IS NULL OR (typeof(provider_turn_id) = 'text'
        AND length(CAST(provider_turn_id AS BLOB)) BETWEEN 1 AND 128)),
    CHECK (typeof(model_id) = 'text'
        AND length(CAST(model_id AS BLOB)) BETWEEN 1 AND 128),
    CHECK (typeof(provider_route_id) = 'text'
        AND length(CAST(provider_route_id AS BLOB)) BETWEEN 1 AND 128),
    CHECK (variant_id IS NULL OR (typeof(variant_id) = 'text'
        AND length(CAST(variant_id AS BLOB)) BETWEEN 1 AND 128)),
    CHECK (input_tokens IS NULL OR (typeof(input_tokens) = 'integer'
        AND input_tokens BETWEEN 0 AND 9223372036854775807)),
    CHECK (cached_input_tokens IS NULL OR (typeof(cached_input_tokens) = 'integer'
        AND cached_input_tokens BETWEEN 0 AND 9223372036854775807)),
    CHECK (output_tokens IS NULL OR (typeof(output_tokens) = 'integer'
        AND output_tokens BETWEEN 0 AND 9223372036854775807)),
    CHECK (context_tokens IS NULL OR (typeof(context_tokens) = 'integer'
        AND context_tokens BETWEEN 0 AND 9223372036854775807)),
    CHECK (context_window_tokens IS NULL OR (typeof(context_window_tokens) = 'integer'
        AND context_window_tokens BETWEEN 1 AND 9223372036854775807)),
    CHECK (typeof(observed_at_ms) = 'integer')
)
"#;

/// Creates the bounded current usage record after the model-favorites leaf.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(RUN_USAGE_TABLE)
            .await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE run_usage")
            .await.map(|_| ())
    }
}
