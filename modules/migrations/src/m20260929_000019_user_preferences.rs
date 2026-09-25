//! Adds the Forge user's preferences and navigation record.
//!
//! A Forge serves one account, so its preferences are one singleton row: the
//! default engine configuration new threads start from, the route the user
//! was last on, and a revision that grows with every change. The navigation
//! record keeps one row per used project with its recency (the preferences
//! revision at its last use, so the order needs no clock) and the thread last
//! open in it. Deleting a project or thread clears its references.

use sea_orm_migration::prelude::*;

/// Upper bound of one encoded engine configuration, matching
/// `ENGINE_CONFIG_MAX_ENCODED_BYTES`.
const ENGINE_CONFIG_MAX_BYTES: i64 = 64 * 1024;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE user_preferences (\
                    state_id INTEGER NOT NULL PRIMARY KEY,\
                    revision INTEGER NOT NULL DEFAULT 0,\
                    default_engine_config BLOB NULL,\
                    route_project_id TEXT NULL,\
                    route_thread_id TEXT NULL,\
                    updated_at_ms INTEGER NOT NULL DEFAULT 0,\
                    FOREIGN KEY (route_project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE SET NULL,\
                    FOREIGN KEY (route_thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE SET NULL,\
                    CHECK (state_id = 1),\
                    CHECK (typeof(revision) = 'integer' AND revision >= 0),\
                    CHECK (default_engine_config IS NULL OR (typeof(default_engine_config) = 'blob' AND length(default_engine_config) BETWEEN 1 AND {ENGINE_CONFIG_MAX_BYTES})),\
                    CHECK (route_thread_id IS NULL OR route_project_id IS NOT NULL),\
                    CHECK (typeof(updated_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "INSERT INTO user_preferences (state_id, revision, updated_at_ms) VALUES (1, 0, 0)",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER ck_user_preferences_revision_monotonic BEFORE UPDATE OF revision ON user_preferences WHEN NEW.revision < OLD.revision BEGIN SELECT RAISE(ABORT, 'user preferences revision cannot decrease'); END",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TABLE navigation_projects (\
                    project_id TEXT NOT NULL PRIMARY KEY,\
                    recency INTEGER NOT NULL,\
                    last_thread_id TEXT NULL,\
                    FOREIGN KEY (project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE CASCADE,\
                    FOREIGN KEY (last_thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE SET NULL,\
                    CHECK (typeof(recency) = 'integer' AND recency >= 0)\
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_navigation_projects_recency ON navigation_projects(recency DESC, project_id)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for statement in [
            "DROP INDEX IF EXISTS idx_navigation_projects_recency",
            "DROP TABLE IF EXISTS navigation_projects",
            "DROP TRIGGER IF EXISTS ck_user_preferences_revision_monotonic",
            "DROP TABLE IF EXISTS user_preferences",
        ] {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
