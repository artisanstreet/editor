//! Lets the composer attachment store hold images up to 32 MiB, uploaded in
//! chunks.
//!
//! A picked image larger than one transport frame crosses the wire in
//! chunks. Each chunk waits in `composer_attachment_upload_chunks`, keyed by
//! the digest of the whole image and its offset, until every byte arrived;
//! the Forge then assembles the image, checks its digest, stores it, and
//! drops the chunks. Chunks of an upload that never completes are pruned
//! after the attachment grace period. The store's size CHECK is relaxed by
//! rebuilding both attachment tables, as migration 18 did.

use sea_orm_migration::prelude::*;

use super::m20260928_000018_composer_attachment_sources::rebuild;

const ATTACHMENT_MAX_BYTES: i64 = 32 * 1024 * 1024;
const PREVIOUS_ATTACHMENT_MAX_BYTES: i64 = 12 * 1024 * 1024;
const CHUNK_MAX_BYTES: i64 = 4 * 1024 * 1024;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        rebuild(connection, ATTACHMENT_MAX_BYTES, None).await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE composer_attachment_upload_chunks (\
                    digest BLOB NOT NULL,\
                    offset_bytes INTEGER NOT NULL,\
                    mime_type TEXT NOT NULL,\
                    total_bytes INTEGER NOT NULL,\
                    bytes BLOB NOT NULL,\
                    stored_at_ms INTEGER NOT NULL,\
                    PRIMARY KEY (digest, offset_bytes),\
                    CHECK (typeof(digest) = 'blob' AND length(digest) = 32),\
                    CHECK (mime_type IN ('image/gif', 'image/jpeg', 'image/png', 'image/webp')),\
                    CHECK (typeof(total_bytes) = 'integer' AND total_bytes BETWEEN 1 AND {ATTACHMENT_MAX_BYTES}),\
                    CHECK (typeof(offset_bytes) = 'integer' AND offset_bytes >= 0 AND offset_bytes < total_bytes),\
                    CHECK (typeof(bytes) = 'blob' AND length(bytes) BETWEEN 1 AND {CHUNK_MAX_BYTES}),\
                    CHECK (offset_bytes + length(bytes) <= total_bytes),\
                    CHECK (typeof(stored_at_ms) = 'integer')\
                )"
            ))
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP TABLE IF EXISTS composer_attachment_upload_chunks")
            .await?;
        // Stored images over the previous bound cannot be kept by the older
        // schema; references to them are dropped with them.
        rebuild(
            connection,
            PREVIOUS_ATTACHMENT_MAX_BYTES,
            Some(PREVIOUS_ATTACHMENT_MAX_BYTES),
        )
        .await
    }
}
