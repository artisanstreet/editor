//! Optional observed text-stream rate. Old rows retain NULL: no timing is invented.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(
            "ALTER TABLE run_usage ADD COLUMN streaming_millitokens_per_second INTEGER NULL CHECK (streaming_millitokens_per_second IS NULL OR (typeof(streaming_millitokens_per_second) = 'integer' AND streaming_millitokens_per_second BETWEEN 1 AND 4294967295))"
        ).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE run_usage DROP COLUMN streaming_millitokens_per_second",
            )
            .await
            .map(|_| ())
    }
}
