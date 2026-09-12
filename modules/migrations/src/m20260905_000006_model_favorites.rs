//! Adds the globally scoped durable model-favorites preference.
//!
//! Favorites are kept separate from the shared command-receipt table. The
//! separate receipt table stores the exact result snapshot for each request,
//! which lets a retry return its original result after later mutations.

use sea_orm_migration::prelude::*;

const REQUEST_ID_MAX_BYTES: i64 = 128;
const MODEL_FAVORITE_ID_MAX_BYTES: i64 = 4_096;
const MODEL_FAVORITES_MAX_MODELS: i64 = 1_024;
const SNAPSHOT_JSON_MAX_BYTES: i64 = 8 * 1024 * 1024;

/// Creates the model-favorites rows, singleton revision, and idempotency
/// receipts.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE model_favorites (\
                    model_id TEXT NOT NULL PRIMARY KEY,\
                    favorited_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(model_id) = 'text' AND length(CAST(model_id AS BLOB)) BETWEEN 1 AND {MODEL_FAVORITE_ID_MAX_BYTES}),\
                    CHECK (typeof(favorited_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_model_favorites_order ON model_favorites(favorited_at_ms, model_id)",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TABLE model_favorites_state (\
                    state_id INTEGER NOT NULL PRIMARY KEY,\
                    revision INTEGER NOT NULL DEFAULT 0,\
                    updated_at_ms INTEGER NOT NULL DEFAULT 0,\
                    CHECK (state_id = 1),\
                    CHECK (typeof(revision) = 'integer' AND revision BETWEEN 0 AND 9223372036854775807),\
                    CHECK (typeof(updated_at_ms) = 'integer')\
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "INSERT INTO model_favorites_state (state_id, revision, updated_at_ms) VALUES (1, 0, 0)",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE model_favorite_receipts (\
                    request_id TEXT NOT NULL PRIMARY KEY,\
                    model_id TEXT NOT NULL,\
                    favorite INTEGER NOT NULL,\
                    result_revision INTEGER NOT NULL,\
                    snapshot_json TEXT NOT NULL,\
                    accepted_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(request_id) = 'text' AND length(CAST(request_id AS BLOB)) BETWEEN 1 AND {REQUEST_ID_MAX_BYTES}),\
                    CHECK (typeof(model_id) = 'text' AND length(CAST(model_id AS BLOB)) BETWEEN 1 AND {MODEL_FAVORITE_ID_MAX_BYTES}),\
                    CHECK (typeof(favorite) = 'integer' AND favorite IN (0, 1)),\
                    CHECK (typeof(result_revision) = 'integer' AND result_revision BETWEEN 0 AND 9223372036854775807),\
                    CHECK (typeof(snapshot_json) = 'text' AND length(CAST(snapshot_json AS BLOB)) BETWEEN 2 AND {SNAPSHOT_JSON_MAX_BYTES}),\
                    CHECK (typeof(accepted_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_model_favorite_receipts_model_id ON model_favorite_receipts(model_id)",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TRIGGER ck_model_favorites_max_insert BEFORE INSERT ON model_favorites WHEN NOT EXISTS (SELECT 1 FROM model_favorites WHERE model_id = NEW.model_id) AND (SELECT count(*) FROM model_favorites) >= {MODEL_FAVORITES_MAX_MODELS} BEGIN SELECT RAISE(ABORT, 'model favorites maximum exceeded'); END"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER ck_model_favorites_revision_monotonic BEFORE UPDATE OF revision ON model_favorites_state WHEN NEW.revision < OLD.revision BEGIN SELECT RAISE(ABORT, 'model favorites revision cannot decrease'); END",
            )
            .await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS ck_model_favorites_max_insert")
            .await?;
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS ck_model_favorites_revision_monotonic")
            .await?;
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_model_favorite_receipts_model_id")
            .await?;
        connection
            .execute_unprepared("DROP TABLE IF EXISTS model_favorite_receipts")
            .await?;
        connection
            .execute_unprepared("DROP TABLE IF EXISTS model_favorites_state")
            .await?;
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_model_favorites_order")
            .await?;
        connection
            .execute_unprepared("DROP TABLE IF EXISTS model_favorites")
            .await
            .map(|_| ())
    }
}
