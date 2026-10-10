//! Adds the options the user last chose for each model.
//!
//! One row per catalog model holds the engine configuration the user last
//! chose it with, and its recency (the preferences revision of that choice),
//! so picking a model again starts from the user's own options rather than
//! the catalog defaults. The last model chosen stays the default engine
//! configuration on `user_preferences`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE model_preferences (\
                    model_id TEXT NOT NULL PRIMARY KEY,\
                    engine_config BLOB NOT NULL,\
                    recency INTEGER NOT NULL,\
                    CHECK (length(model_id) BETWEEN 1 AND 4096),\
                    CHECK (typeof(engine_config) = 'blob' AND length(engine_config) BETWEEN 1 AND 65536),\
                    CHECK (typeof(recency) = 'integer' AND recency >= 0)\
                )",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS model_preferences")
            .await
            .map(|_| ())
    }
}
