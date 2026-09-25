//! Adds Forge-owned composer drafts and the content-addressed attachment
//! store they reference.
//!
//! `composer_attachments` stores each encoded image once under the SHA-256
//! digest of its bytes. `composer_drafts` holds one row per composer scope
//! (a thread, or a project's new-task composer) with a Forge-assigned
//! revision that may only grow; an emptied draft keeps its row, so its
//! revision sequence never restarts. `composer_draft_attachments` lists a
//! draft's references in authored order and pins the stored bytes through a
//! restricting foreign key.

use sea_orm_migration::prelude::*;

const SCOPE_ID_MAX_BYTES: i64 = 128;
const BODY_MAX_BYTES: i64 = 65_536;
const ATTACHMENT_MAX_BYTES: i64 = 5 * 1024 * 1024;
const ATTACHMENT_MAX_COUNT: i64 = 10;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE composer_attachments (\
                    digest BLOB NOT NULL PRIMARY KEY,\
                    mime_type TEXT NOT NULL,\
                    size_bytes INTEGER NOT NULL,\
                    bytes BLOB NOT NULL,\
                    stored_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(digest) = 'blob' AND length(digest) = 32),\
                    CHECK (mime_type IN ('image/gif', 'image/jpeg', 'image/png', 'image/webp')),\
                    CHECK (typeof(size_bytes) = 'integer' AND size_bytes BETWEEN 1 AND {ATTACHMENT_MAX_BYTES}),\
                    CHECK (typeof(bytes) = 'blob' AND length(bytes) = size_bytes),\
                    CHECK (typeof(stored_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE composer_drafts (\
                    scope_kind TEXT NOT NULL,\
                    scope_id TEXT NOT NULL,\
                    revision INTEGER NOT NULL,\
                    body TEXT NOT NULL,\
                    updated_at_ms INTEGER NOT NULL,\
                    PRIMARY KEY (scope_kind, scope_id),\
                    CHECK (scope_kind IN ('thread', 'project')),\
                    CHECK (typeof(scope_id) = 'text' AND length(CAST(scope_id AS BLOB)) BETWEEN 1 AND {SCOPE_ID_MAX_BYTES}),\
                    CHECK (typeof(revision) = 'integer' AND revision BETWEEN 1 AND 9223372036854775807),\
                    CHECK (typeof(body) = 'text' AND length(CAST(body AS BLOB)) <= {BODY_MAX_BYTES}),\
                    CHECK (typeof(updated_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER ck_composer_drafts_revision_increases BEFORE UPDATE OF revision ON composer_drafts WHEN NEW.revision <= OLD.revision BEGIN SELECT RAISE(ABORT, 'composer draft revision must increase'); END",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE composer_draft_attachments (\
                    scope_kind TEXT NOT NULL,\
                    scope_id TEXT NOT NULL,\
                    position INTEGER NOT NULL,\
                    digest BLOB NOT NULL,\
                    name TEXT NOT NULL,\
                    PRIMARY KEY (scope_kind, scope_id, position),\
                    FOREIGN KEY (scope_kind, scope_id) REFERENCES composer_drafts(scope_kind, scope_id) ON UPDATE RESTRICT ON DELETE CASCADE,\
                    FOREIGN KEY (digest) REFERENCES composer_attachments(digest) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(position) = 'integer' AND position BETWEEN 0 AND {max_position}),\
                    CHECK (typeof(name) = 'text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 256)\
                )",
                max_position = ATTACHMENT_MAX_COUNT - 1
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_composer_draft_attachments_digest ON composer_draft_attachments(digest)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for statement in [
            "DROP INDEX IF EXISTS idx_composer_draft_attachments_digest",
            "DROP TABLE IF EXISTS composer_draft_attachments",
            "DROP TRIGGER IF EXISTS ck_composer_drafts_revision_increases",
            "DROP TABLE IF EXISTS composer_drafts",
            "DROP TABLE IF EXISTS composer_attachments",
        ] {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
