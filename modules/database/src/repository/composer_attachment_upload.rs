//! Chunked uploads into, and windowed reads out of, the composer attachment
//! store.
//!
//! A picked image larger than one transport frame arrives in chunks keyed by
//! the digest of the whole image and each chunk's offset. Chunks wait in
//! `composer_attachment_upload_chunks` until they tile the image; the Forge
//! then assembles it, checks the digest, stores it once, and drops the
//! chunks. A chunk that contradicts the upload it joins, or an assembled
//! image whose digest disagrees, discards the upload. Chunks of an upload
//! that never completes are pruned after the unreferenced-attachment grace
//! period.

use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement, TryGetable, Value,
};
use sha2::{Digest, Sha256};

use artisan_domain::{
    ComposerAttachmentChunk, ComposerAttachmentDigest, ComposerAttachmentRef,
    ComposerAttachmentResult, ReadComposerAttachment, UnixMillis,
};

use super::composer_draft::{
    COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS, ComposerDraftRepositoryError, parse_mime,
};
use super::{Repository, corrupt_data, database_error};

type UploadResult<T> = Result<T, ComposerDraftRepositoryError>;

const TABLE: &str = "composer_attachment_upload_chunks";

/// Where one chunked upload stands after a chunk arrived.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerAttachmentChunkOutcome {
    /// Reference naming the image under the uploaded display name.
    pub reference: ComposerAttachmentRef,
    /// Bytes still missing; zero once the image is stored.
    pub pending_bytes: u32,
}

impl Repository {
    /// Accepts one chunk of a picked image and stores the image once every
    /// byte arrived and its digest matches.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftRepositoryError::ChunkRejected`] for a chunk
    /// that contradicts its upload or completes an image whose digest
    /// disagrees (the upload is discarded),
    /// [`ComposerDraftRepositoryError::AttachmentMismatch`] when the digest
    /// is already stored with other metadata, or a database error.
    pub async fn store_composer_attachment_chunk(
        &self,
        chunk: &ComposerAttachmentChunk,
        stored_at: UnixMillis,
    ) -> UploadResult<ComposerAttachmentChunkOutcome> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin composer attachment chunk", source))?;
        let applied = apply_chunk(&transaction, chunk, stored_at).await;
        // A rejected chunk still commits: discarding the upload is its
        // outcome, and the pruning is harmless.
        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit composer attachment chunk", source))?;
        let pending_bytes = applied?;
        let reference = ComposerAttachmentRef::new(
            *chunk.digest(),
            chunk.mime_type(),
            chunk.name(),
            chunk.total_bytes(),
        )
        .map_err(|source| corrupt_data(TABLE, "name", source))?;
        Ok(ComposerAttachmentChunkOutcome {
            reference,
            pending_bytes,
        })
    }

    /// Reads a window of one stored attachment's bytes, or `None` when the
    /// digest is not stored.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftRepositoryError::AttachmentMismatch`] for a
    /// window that starts past the image's end, or an error when the row is
    /// corrupt or `SQLite` fails.
    pub async fn read_composer_attachment_window(
        &self,
        read: &ReadComposerAttachment,
    ) -> UploadResult<Option<ComposerAttachmentResult>> {
        let length = if read.max_bytes == 0 {
            i64::from(u32::MAX)
        } else {
            i64::from(read.max_bytes)
        };
        let row = self
            .database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT mime_type, size_bytes, substr(bytes, ?, ?) FROM composer_attachments WHERE digest = ?",
                [
                    Value::BigInt(Some(i64::from(read.offset) + 1)),
                    Value::BigInt(Some(length)),
                    digest_value(&read.digest),
                ],
            ))
            .await
            .map_err(|source| database_error("read composer attachment window", source))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let table = "composer_attachments";
        let mime_type = parse_mime(
            &row.try_get_by_index::<String>(0)
                .map_err(|source| corrupt_data(table, "mime_type", source))?,
        )?;
        let total_bytes = row
            .try_get_by_index::<i64>(1)
            .map_err(|source| corrupt_data(table, "size_bytes", source))?;
        let total_bytes = u32::try_from(total_bytes)
            .map_err(|_| corrupt_data(table, "size_bytes", "out of range"))?;
        let bytes = row
            .try_get_by_index::<Vec<u8>>(2)
            .map_err(|source| corrupt_data(table, "bytes", source))?;
        if read.offset >= total_bytes || bytes.is_empty() {
            return Err(ComposerDraftRepositoryError::AttachmentMismatch {
                digest: read.digest,
            });
        }
        Ok(Some(ComposerAttachmentResult {
            digest: read.digest,
            mime_type,
            bytes,
            total_bytes,
            offset: read.offset,
        }))
    }
}

/// Records one chunk inside `transaction` and answers the bytes still
/// missing.
async fn apply_chunk(
    transaction: &DatabaseTransaction,
    chunk: &ComposerAttachmentChunk,
    stored_at: UnixMillis,
) -> UploadResult<u32> {
    let digest = digest_value(chunk.digest());
    let stale_before = stored_at
        .as_millis()
        .saturating_sub(COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS);
    execute(
        transaction,
        "DELETE FROM composer_attachment_upload_chunks WHERE stored_at_ms < ?",
        vec![Value::BigInt(Some(stale_before))],
        "prune stale composer attachment chunks",
    )
    .await?;
    if let Some((mime_type, size)) = stored_metadata(transaction, chunk.digest()).await? {
        if mime_type != chunk.mime_type().as_str() || size != chunk.total_bytes() {
            return Err(ComposerDraftRepositoryError::AttachmentMismatch {
                digest: *chunk.digest(),
            });
        }
        execute(
            transaction,
            "UPDATE composer_attachments SET stored_at_ms = max(stored_at_ms, ?) WHERE digest = ?",
            vec![Value::BigInt(Some(stored_at.as_millis())), digest],
            "refresh stored composer attachment",
        )
        .await?;
        return Ok(0);
    }
    execute(
        transaction,
        "INSERT INTO composer_attachment_upload_chunks (digest, offset_bytes, mime_type, total_bytes, bytes, stored_at_ms) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(digest, offset_bytes) DO UPDATE SET bytes = excluded.bytes, mime_type = excluded.mime_type, total_bytes = excluded.total_bytes",
        vec![
            digest.clone(),
            Value::BigInt(Some(i64::from(chunk.offset()))),
            Value::String(Some(chunk.mime_type().as_str().to_owned())),
            Value::BigInt(Some(i64::from(chunk.total_bytes()))),
            Value::Bytes(Some(chunk.bytes().to_vec())),
            Value::BigInt(Some(stored_at.as_millis())),
        ],
        "store composer attachment chunk",
    )
    .await?;
    execute(
        transaction,
        "UPDATE composer_attachment_upload_chunks SET stored_at_ms = ? WHERE digest = ?",
        vec![Value::BigInt(Some(stored_at.as_millis())), digest.clone()],
        "refresh composer attachment chunks",
    )
    .await?;
    let rows = transaction
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT offset_bytes, length(bytes), mime_type, total_bytes FROM composer_attachment_upload_chunks WHERE digest = ? ORDER BY offset_bytes ASC",
            [digest.clone()],
        ))
        .await
        .map_err(|source| database_error("read composer attachment chunks", source))?;
    let mut end = 0_i64;
    let mut received = 0_i64;
    let mut tiled = true;
    for row in &rows {
        let offset = column::<i64>(row, 0, "offset_bytes")?;
        let length = column::<i64>(row, 1, "bytes")?;
        let mime_type = column::<String>(row, 2, "mime_type")?;
        let total = column::<i64>(row, 3, "total_bytes")?;
        let consistent =
            mime_type == chunk.mime_type().as_str() && total == i64::from(chunk.total_bytes());
        if !consistent || offset < end {
            discard(transaction, digest).await?;
            return Err(ComposerDraftRepositoryError::ChunkRejected {
                digest: *chunk.digest(),
            });
        }
        tiled &= offset == end;
        end = offset + length;
        received += length;
    }
    let pending = i64::from(chunk.total_bytes()) - received;
    if pending > 0 || !tiled {
        return Ok(u32::try_from(pending.max(1)).unwrap_or(u32::MAX));
    }
    assemble(transaction, chunk, stored_at).await.map(|()| 0)
}

/// Joins every chunk of a complete upload, checks the digest, and stores
/// the image.
async fn assemble(
    transaction: &DatabaseTransaction,
    chunk: &ComposerAttachmentChunk,
    stored_at: UnixMillis,
) -> UploadResult<()> {
    let digest = digest_value(chunk.digest());
    let rows = transaction
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT bytes FROM composer_attachment_upload_chunks WHERE digest = ? ORDER BY offset_bytes ASC",
            [digest.clone()],
        ))
        .await
        .map_err(|source| database_error("assemble composer attachment", source))?;
    let mut image = Vec::with_capacity(usize::try_from(chunk.total_bytes()).unwrap_or(0));
    for row in &rows {
        image.extend(column::<Vec<u8>>(row, 0, "bytes")?);
    }
    discard(transaction, digest.clone()).await?;
    let assembled = ComposerAttachmentDigest::new(Sha256::digest(&image).into());
    if &assembled != chunk.digest() {
        return Err(ComposerDraftRepositoryError::ChunkRejected {
            digest: *chunk.digest(),
        });
    }
    execute(
        transaction,
        "INSERT INTO composer_attachments (digest, mime_type, size_bytes, bytes, stored_at_ms) VALUES (?, ?, ?, ?, ?)",
        vec![
            digest,
            Value::String(Some(chunk.mime_type().as_str().to_owned())),
            Value::BigInt(Some(i64::from(chunk.total_bytes()))),
            Value::Bytes(Some(image)),
            Value::BigInt(Some(stored_at.as_millis())),
        ],
        "store assembled composer attachment",
    )
    .await
}

async fn stored_metadata(
    transaction: &DatabaseTransaction,
    digest: &ComposerAttachmentDigest,
) -> UploadResult<Option<(String, u32)>> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT mime_type, size_bytes FROM composer_attachments WHERE digest = ?",
            [digest_value(digest)],
        ))
        .await
        .map_err(|source| database_error("read stored composer attachment", source))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let table = "composer_attachments";
    let mime_type = row
        .try_get_by_index::<String>(0)
        .map_err(|source| corrupt_data(table, "mime_type", source))?;
    let size = row
        .try_get_by_index::<i64>(1)
        .map_err(|source| corrupt_data(table, "size_bytes", source))?;
    let size =
        u32::try_from(size).map_err(|_| corrupt_data(table, "size_bytes", "out of range"))?;
    Ok(Some((mime_type, size)))
}

async fn discard(transaction: &DatabaseTransaction, digest: Value) -> UploadResult<()> {
    execute(
        transaction,
        "DELETE FROM composer_attachment_upload_chunks WHERE digest = ?",
        vec![digest],
        "discard composer attachment chunks",
    )
    .await
}

async fn execute(
    transaction: &DatabaseTransaction,
    sql: &str,
    values: Vec<Value>,
    operation: &'static str,
) -> UploadResult<()> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .await
        .map(|_| ())
        .map_err(|source| database_error(operation, source))
}

fn column<T: TryGetable>(row: &QueryResult, index: usize, field: &'static str) -> UploadResult<T> {
    row.try_get_by_index::<T>(index)
        .map_err(|source| corrupt_data(TABLE, field, source))
}

fn digest_value(digest: &ComposerAttachmentDigest) -> Value {
    Value::Bytes(Some(digest.as_bytes().to_vec()))
}
