//! Adds Artisan-managed extensions and the user's global prompt.
//!
//! `extension_installations` holds one row per managed skill: Artisan's own
//! skills once the user changed them from their default, and every skill the
//! user adopted into the content store. Engines know skills by name, so a
//! name is managed at most once.
//!
//! `global_prompt` is a singleton holding the canonical prompt, whether it is
//! established (setup imported or chose it), and a revision that grows with
//! every saved version; `global_prompt_versions` keeps the saved versions.
//! `global_prompt_engine_files` records the digest Artisan last wrote to each
//! engine home's global file, so an edit made outside Artisan is recognised
//! as drift instead of being overwritten.

use sea_orm_migration::prelude::*;

/// Upper bound of one prompt text, matching `GLOBAL_PROMPT_MAX_BYTES`.
const GLOBAL_PROMPT_MAX_BYTES: i64 = 64 * 1024;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "CREATE TABLE extension_installations (\
                    extension_id TEXT NOT NULL PRIMARY KEY,\
                    origin TEXT NOT NULL,\
                    name TEXT NOT NULL,\
                    digest TEXT NOT NULL,\
                    enabled INTEGER NOT NULL,\
                    updated_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(extension_id) = 'text' AND length(extension_id) BETWEEN 1 AND 160),\
                    CHECK (origin IN ('first_party', 'adopted')),\
                    CHECK (typeof(name) = 'text' AND length(name) BETWEEN 1 AND 64),\
                    CHECK (typeof(digest) = 'text' AND length(digest) = 64),\
                    CHECK (enabled IN (0, 1)),\
                    CHECK (typeof(updated_at_ms) = 'integer')\
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE UNIQUE INDEX ux_extension_installations_name ON extension_installations(name)",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE global_prompt (\
                    state_id INTEGER NOT NULL PRIMARY KEY,\
                    revision INTEGER NOT NULL DEFAULT 0,\
                    established INTEGER NOT NULL DEFAULT 0,\
                    text TEXT NOT NULL DEFAULT '',\
                    updated_at_ms INTEGER NOT NULL DEFAULT 0,\
                    CHECK (state_id = 1),\
                    CHECK (typeof(revision) = 'integer' AND revision >= 0),\
                    CHECK (established IN (0, 1)),\
                    CHECK (typeof(text) = 'text' AND length(CAST(text AS BLOB)) <= {GLOBAL_PROMPT_MAX_BYTES}),\
                    CHECK (typeof(updated_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "INSERT INTO global_prompt (state_id, revision, established, text, updated_at_ms) VALUES (1, 0, 0, '', 0)",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER ck_global_prompt_revision_monotonic BEFORE UPDATE OF revision ON global_prompt WHEN NEW.revision < OLD.revision BEGIN SELECT RAISE(ABORT, 'global prompt revision cannot decrease'); END",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE global_prompt_versions (\
                    revision INTEGER NOT NULL PRIMARY KEY,\
                    text TEXT NOT NULL,\
                    saved_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(revision) = 'integer' AND revision >= 1),\
                    CHECK (typeof(text) = 'text' AND length(CAST(text AS BLOB)) <= {GLOBAL_PROMPT_MAX_BYTES}),\
                    CHECK (typeof(saved_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE TABLE global_prompt_engine_files (\
                    engine_id TEXT NOT NULL,\
                    home_key TEXT NOT NULL,\
                    written_digest TEXT NOT NULL,\
                    written_at_ms INTEGER NOT NULL,\
                    PRIMARY KEY (engine_id, home_key),\
                    CHECK (typeof(engine_id) = 'text' AND length(engine_id) BETWEEN 1 AND 64),\
                    CHECK (typeof(home_key) = 'text' AND length(home_key) BETWEEN 1 AND 512),\
                    CHECK (typeof(written_digest) = 'text' AND length(written_digest) = 64),\
                    CHECK (typeof(written_at_ms) = 'integer')\
                )",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for statement in [
            "DROP TABLE IF EXISTS global_prompt_engine_files",
            "DROP TABLE IF EXISTS global_prompt_versions",
            "DROP TRIGGER IF EXISTS ck_global_prompt_revision_monotonic",
            "DROP TABLE IF EXISTS global_prompt",
            "DROP INDEX IF EXISTS ux_extension_installations_name",
            "DROP TABLE IF EXISTS extension_installations",
        ] {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
