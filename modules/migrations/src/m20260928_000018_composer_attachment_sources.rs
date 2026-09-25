//! Lets the composer attachment store hold images as the user picked them.
//!
//! The Editor used to rescale and re-encode every image for the engine it
//! expected before uploading it, so the store's bound was the message image
//! bound (5 MiB). The Forge now owns that image policy and applies it when a
//! draft is sent, so the store keeps the picked image (up to 12 MiB, one
//! upload frame). SQLite cannot relax a CHECK in place: both attachment
//! tables are rebuilt. The draft-attachment table is rebuilt first against
//! the new store so its restricting foreign key never points at a dropped
//! table; renaming the new store then carries that reference to its final
//! name.

use sea_orm_migration::prelude::*;

const ATTACHMENT_MAX_BYTES: i64 = 12 * 1024 * 1024;
const PREVIOUS_ATTACHMENT_MAX_BYTES: i64 = 5 * 1024 * 1024;
const ATTACHMENT_MAX_COUNT: i64 = 10;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(manager.get_connection(), ATTACHMENT_MAX_BYTES, None).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Stored images over the previous bound cannot be kept by the older
        // schema; references to them are dropped with them.
        rebuild(
            manager.get_connection(),
            PREVIOUS_ATTACHMENT_MAX_BYTES,
            Some(PREVIOUS_ATTACHMENT_MAX_BYTES),
        )
        .await
    }
}

pub(super) async fn rebuild(
    connection: &SchemaManagerConnection<'_>,
    max_bytes: i64,
    keep_at_most: Option<i64>,
) -> Result<(), DbErr> {
    let keep =
        keep_at_most.map_or_else(String::new, |bound| format!(" WHERE size_bytes <= {bound}"));
    for statement in [
        format!(
            "CREATE TABLE composer_attachments_rebuilt (\
                digest BLOB NOT NULL PRIMARY KEY,\
                mime_type TEXT NOT NULL,\
                size_bytes INTEGER NOT NULL,\
                bytes BLOB NOT NULL,\
                stored_at_ms INTEGER NOT NULL,\
                CHECK (typeof(digest) = 'blob' AND length(digest) = 32),\
                CHECK (mime_type IN ('image/gif', 'image/jpeg', 'image/png', 'image/webp')),\
                CHECK (typeof(size_bytes) = 'integer' AND size_bytes BETWEEN 1 AND {max_bytes}),\
                CHECK (typeof(bytes) = 'blob' AND length(bytes) = size_bytes),\
                CHECK (typeof(stored_at_ms) = 'integer')\
            )"
        ),
        format!(
            "INSERT INTO composer_attachments_rebuilt (digest, mime_type, size_bytes, bytes, stored_at_ms) \
             SELECT digest, mime_type, size_bytes, bytes, stored_at_ms FROM composer_attachments{keep}"
        ),
        format!(
            "CREATE TABLE composer_draft_attachments_rebuilt (\
                scope_kind TEXT NOT NULL,\
                scope_id TEXT NOT NULL,\
                position INTEGER NOT NULL,\
                digest BLOB NOT NULL,\
                name TEXT NOT NULL,\
                PRIMARY KEY (scope_kind, scope_id, position),\
                FOREIGN KEY (scope_kind, scope_id) REFERENCES composer_drafts(scope_kind, scope_id) ON UPDATE RESTRICT ON DELETE CASCADE,\
                FOREIGN KEY (digest) REFERENCES composer_attachments_rebuilt(digest) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                CHECK (typeof(position) = 'integer' AND position BETWEEN 0 AND {max_position}),\
                CHECK (typeof(name) = 'text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 256)\
            )",
            max_position = ATTACHMENT_MAX_COUNT - 1
        ),
        "INSERT INTO composer_draft_attachments_rebuilt (scope_kind, scope_id, position, digest, name) \
         SELECT scope_kind, scope_id, position, digest, name FROM composer_draft_attachments \
         WHERE digest IN (SELECT digest FROM composer_attachments_rebuilt)"
            .to_owned(),
        "DROP INDEX IF EXISTS idx_composer_draft_attachments_digest".to_owned(),
        "DROP TABLE composer_draft_attachments".to_owned(),
        "DROP TABLE composer_attachments".to_owned(),
        "ALTER TABLE composer_attachments_rebuilt RENAME TO composer_attachments".to_owned(),
        "ALTER TABLE composer_draft_attachments_rebuilt RENAME TO composer_draft_attachments"
            .to_owned(),
        "CREATE INDEX idx_composer_draft_attachments_digest ON composer_draft_attachments(digest)"
            .to_owned(),
    ] {
        connection.execute_unprepared(&statement).await?;
    }
    Ok(())
}
