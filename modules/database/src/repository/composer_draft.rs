//! Forge-owned composer drafts and the content-addressed attachment store.
//!
//! Every draft save applies: the last save to arrive wins, and the Forge
//! assigns it the next revision of its scope. A save request that arrives
//! again (a retransmission) answers the revision it was first given and
//! writes nothing, so it cannot bring back a draft that was since sent.
//! Attachment bytes are stored once per SHA-256 digest and pinned by every
//! draft that references them. Unreferenced attachments and save receipts
//! are pruned after a grace period, long enough for a send that names them
//! to resolve.

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, Statement, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use artisan_domain::{
    AuthoredText, ComposerAttachmentDigest, ComposerAttachmentRef, ComposerAttachmentResult,
    ComposerDraft, ComposerDraftRevision, ComposerDraftScope, ComposerImage, ImageAttachment,
    ImageMimeType, RequestId, UnixMillis,
};

use super::{Repository, RepositoryFailure, corrupt_data, database_error};

/// How long an attachment no draft references stays resolvable, in
/// milliseconds. A send resolves its references within one request timeout.
pub const COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS: i64 = 24 * 60 * 60 * 1000;

const DRAFT_SELECT: &str = "SELECT revision, body, updated_at_ms FROM composer_drafts WHERE scope_kind = ? AND scope_id = ?";
const DRAFT_ATTACHMENTS_SELECT: &str = "SELECT d.digest, d.name, a.mime_type, a.size_bytes FROM composer_draft_attachments d JOIN composer_attachments a ON a.digest = d.digest WHERE d.scope_kind = ? AND d.scope_id = ? ORDER BY d.position ASC";
const ATTACHMENT_META_SELECT: &str =
    "SELECT mime_type, size_bytes FROM composer_attachments WHERE digest = ?";
const ATTACHMENT_SELECT: &str =
    "SELECT mime_type, bytes FROM composer_attachments WHERE digest = ?";

/// Input for one draft save.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveComposerDraftInput {
    /// Save request identity; a repeated request answers its first revision.
    pub request_id: RequestId,
    /// Draft scope.
    pub scope: ComposerDraftScope,
    /// Authored text.
    pub text: AuthoredText,
    /// Stored attachments in authored order.
    pub attachments: Vec<ComposerAttachmentRef>,
    /// Forge acceptance instant.
    pub saved_at: UnixMillis,
}

/// Outcome of one draft save.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComposerDraftSaveOutcome {
    /// Revision the Forge assigned to this save.
    pub revision: ComposerDraftRevision,
}

/// Failure at the composer-draft persistence boundary.
#[derive(Debug, Error)]
pub enum ComposerDraftRepositoryError {
    /// The scoped thread or project does not exist.
    #[error("composer draft scope {kind} `{id}` does not exist")]
    ScopeNotFound {
        /// Scope kind.
        kind: &'static str,
        /// Scoped identity.
        id: String,
    },
    /// A reference names bytes the store does not hold.
    #[error("composer attachment {digest} is not stored")]
    AttachmentNotStored {
        /// Missing digest.
        digest: ComposerAttachmentDigest,
    },
    /// A reference disagrees with the stored MIME type or size.
    #[error("composer attachment {digest} does not match its stored metadata")]
    AttachmentMismatch {
        /// Mismatched digest.
        digest: ComposerAttachmentDigest,
    },
    /// A stored image is larger than a message image; only a draft send,
    /// which fits it to the thread's engine, can use it.
    #[error("composer attachment {digest} is too large to send as it is")]
    AttachmentNotSendable {
        /// Oversized digest.
        digest: ComposerAttachmentDigest,
    },
    /// Persisted data violates a domain or schema invariant.
    #[error("persisted composer draft data is corrupt in {table}.{field}: {reason}")]
    CorruptData {
        /// Table containing the invalid value.
        table: &'static str,
        /// Column containing the invalid value.
        field: &'static str,
        /// Diagnostic explanation.
        reason: String,
    },
    /// The scope's revision cannot advance within its SQLite representation.
    #[error("composer draft revision cannot advance")]
    RevisionExhausted,
    /// A database operation failed.
    #[error("database operation `{operation}` failed")]
    Database {
        /// Operation being attempted.
        operation: &'static str,
        /// Original database error.
        #[source]
        source: DbErr,
    },
}

impl RepositoryFailure for ComposerDraftRepositoryError {
    fn corrupt_data(table: &'static str, field: &'static str, reason: String) -> Self {
        Self::CorruptData {
            table,
            field,
            reason,
        }
    }

    fn database_error(operation: &'static str, source: DbErr) -> Self {
        Self::Database { operation, source }
    }
}

type DraftResult<T> = Result<T, ComposerDraftRepositoryError>;

impl Repository {
    /// Reads one scope's stored draft, or `None` when it never saved one.
    ///
    /// # Errors
    ///
    /// Returns an error when stored rows are corrupt or `SQLite` fails.
    pub async fn read_composer_draft(
        &self,
        scope: &ComposerDraftScope,
    ) -> DraftResult<Option<ComposerDraft>> {
        read_draft(&self.database, scope).await
    }

    /// Replaces one scope's draft and assigns it the scope's next revision.
    ///
    /// The transaction begins `IMMEDIATE`, so concurrent saves of the same
    /// scope serialize: the last to arrive wins and every save gets its own
    /// revision. Every reference must name stored bytes with matching
    /// metadata.
    ///
    /// # Errors
    ///
    /// Returns an unknown-scope or attachment error, or a database error. A
    /// rejected save rolls back.
    pub async fn save_composer_draft(
        &self,
        input: SaveComposerDraftInput,
    ) -> DraftResult<ComposerDraftSaveOutcome> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin composer-draft save", source))?;
        match apply_save(&transaction, &input).await {
            Ok(outcome) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit composer-draft save", source))?;
                Ok(outcome)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|source| database_error("rollback composer-draft save", source))?;
                Err(error)
            }
        }
    }

    /// Stores one image under the digest of its bytes and returns its
    /// reference under the uploaded display name. Storing the same bytes
    /// again is idempotent and refreshes the unreferenced grace period.
    ///
    /// # Errors
    ///
    /// Returns a database error when `SQLite` fails.
    pub async fn store_composer_attachment(
        &self,
        image: &ComposerImage,
        stored_at: UnixMillis,
    ) -> DraftResult<ComposerAttachmentRef> {
        let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
        let size_bytes = u32::try_from(image.byte_len())
            .map_err(|_| corrupt_data("composer_attachments", "size_bytes", "image too large"))?;
        self.database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO composer_attachments (digest, mime_type, size_bytes, bytes, stored_at_ms) VALUES (?, ?, ?, ?, ?) \
                 ON CONFLICT(digest) DO UPDATE SET stored_at_ms = max(stored_at_ms, excluded.stored_at_ms)",
                [
                    Value::Bytes(Some(digest.as_bytes().to_vec())),
                    Value::String(Some(image.mime_type_str().to_owned())),
                    Value::BigInt(Some(i64::from(size_bytes))),
                    Value::Bytes(Some(image.bytes().to_vec())),
                    Value::BigInt(Some(stored_at.as_millis())),
                ],
            ))
            .await
            .map_err(|source| database_error("store composer attachment", source))?;
        ComposerAttachmentRef::new(digest, image.mime_type(), image.name(), size_bytes)
            .map_err(|source| corrupt_data("composer_attachments", "name", source))
    }

    /// Reads the bytes of one stored attachment.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored row is corrupt or `SQLite` fails.
    pub async fn read_composer_attachment(
        &self,
        digest: &ComposerAttachmentDigest,
    ) -> DraftResult<Option<ComposerAttachmentResult>> {
        read_attachment(&self.database, digest).await
    }

    /// Resolves stored references into owned images in authored order, for a
    /// message sent by reference.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftRepositoryError::AttachmentNotStored`] or
    /// [`ComposerDraftRepositoryError::AttachmentMismatch`] for a reference
    /// the store cannot honour, or a database error.
    pub async fn resolve_composer_attachments(
        &self,
        references: &[ComposerAttachmentRef],
    ) -> DraftResult<Vec<ImageAttachment>> {
        resolve_attachments(&self.database, references).await
    }
}

/// Resolves stored references into owned images in authored order.
pub(super) async fn resolve_attachments(
    database: &impl ConnectionTrait,
    references: &[ComposerAttachmentRef],
) -> DraftResult<Vec<ImageAttachment>> {
    let mut images = Vec::with_capacity(references.len());
    for reference in references {
        let stored = read_attachment(database, reference.digest()).await?.ok_or(
            ComposerDraftRepositoryError::AttachmentNotStored {
                digest: *reference.digest(),
            },
        )?;
        let size_matches =
            u32::try_from(stored.bytes.len()).is_ok_and(|size| size == reference.size_bytes());
        if stored.mime_type != reference.mime_type() || !size_matches {
            return Err(ComposerDraftRepositoryError::AttachmentMismatch {
                digest: *reference.digest(),
            });
        }
        images.push(
            ImageAttachment::new(stored.mime_type.as_str(), stored.bytes, reference.name())
                .map_err(|_| ComposerDraftRepositoryError::AttachmentNotSendable {
                    digest: *reference.digest(),
                })?,
        );
    }
    Ok(images)
}

async fn read_attachment(
    database: &impl ConnectionTrait,
    digest: &ComposerAttachmentDigest,
) -> DraftResult<Option<ComposerAttachmentResult>> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            ATTACHMENT_SELECT,
            [Value::Bytes(Some(digest.as_bytes().to_vec()))],
        ))
        .await
        .map_err(|source| database_error("read composer attachment", source))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mime_type = parse_mime(
        &row.try_get_by_index::<String>(0)
            .map_err(|source| corrupt_data("composer_attachments", "mime_type", source))?,
    )?;
    let bytes = row
        .try_get_by_index::<Vec<u8>>(1)
        .map_err(|source| corrupt_data("composer_attachments", "bytes", source))?;
    Ok(Some(ComposerAttachmentResult {
        digest: *digest,
        mime_type,
        bytes,
    }))
}

/// Empties a draft inside `transaction` and gives it the revision after
/// `current`; the row stays, so its revisions never restart.
pub(super) async fn clear_draft(
    transaction: &DatabaseTransaction,
    scope: &ComposerDraftScope,
    current: ComposerDraftRevision,
    cleared_at: UnixMillis,
) -> DraftResult<ComposerDraftRevision> {
    let revision = current
        .next()
        .ok_or(ComposerDraftRepositoryError::RevisionExhausted)?;
    execute(
        transaction,
        "UPDATE composer_drafts SET revision = ?, body = '', updated_at_ms = ? WHERE scope_kind = ? AND scope_id = ?",
        [
            Value::BigInt(Some(revision.as_i64())),
            Value::BigInt(Some(cleared_at.as_millis())),
        ]
        .into_iter()
        .chain(scope_values(scope)),
        "clear submitted composer draft",
    )
    .await?;
    execute(
        transaction,
        "DELETE FROM composer_draft_attachments WHERE scope_kind = ? AND scope_id = ?",
        scope_values(scope),
        "clear submitted composer draft attachments",
    )
    .await?;
    Ok(revision)
}

async fn apply_save(
    transaction: &DatabaseTransaction,
    input: &SaveComposerDraftInput,
) -> DraftResult<ComposerDraftSaveOutcome> {
    ensure_scope_exists(transaction, &input.scope).await?;
    if let Some(revision) = save_receipt(transaction, &input.request_id).await? {
        return Ok(ComposerDraftSaveOutcome { revision });
    }
    let revision = read_revision(transaction, &input.scope)
        .await?
        .unwrap_or_default()
        .next()
        .ok_or(ComposerDraftRepositoryError::RevisionExhausted)?;
    for reference in &input.attachments {
        verify_reference(transaction, reference).await?;
    }
    let scope_values = || scope_values(&input.scope);
    execute(
        transaction,
        "INSERT INTO composer_drafts (scope_kind, scope_id, revision, body, updated_at_ms) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(scope_kind, scope_id) DO UPDATE SET revision = excluded.revision, body = excluded.body, updated_at_ms = excluded.updated_at_ms",
        scope_values().into_iter().chain([
            Value::BigInt(Some(revision.as_i64())),
            Value::String(Some(input.text.as_str().to_owned())),
            Value::BigInt(Some(input.saved_at.as_millis())),
        ]),
        "write composer draft",
    )
    .await?;
    execute(
        transaction,
        "DELETE FROM composer_draft_attachments WHERE scope_kind = ? AND scope_id = ?",
        scope_values(),
        "clear composer draft attachments",
    )
    .await?;
    for (position, reference) in input.attachments.iter().enumerate() {
        let position = i64::try_from(position)
            .map_err(|_| corrupt_data("composer_draft_attachments", "position", "out of range"))?;
        execute(
            transaction,
            "INSERT INTO composer_draft_attachments (scope_kind, scope_id, position, digest, name) VALUES (?, ?, ?, ?, ?)",
            scope_values().into_iter().chain([
                Value::BigInt(Some(position)),
                Value::Bytes(Some(reference.digest().as_bytes().to_vec())),
                Value::String(Some(reference.name().to_owned())),
            ]),
            "write composer draft attachment",
        )
        .await?;
    }
    execute(
        transaction,
        "DELETE FROM composer_attachments WHERE stored_at_ms < ? AND NOT EXISTS \
         (SELECT 1 FROM composer_draft_attachments d WHERE d.digest = composer_attachments.digest)",
        [Value::BigInt(Some(
            input
                .saved_at
                .as_millis()
                .saturating_sub(COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS),
        ))],
        "prune unreferenced composer attachments",
    )
    .await?;
    record_save_receipt(transaction, input, revision).await?;
    Ok(ComposerDraftSaveOutcome { revision })
}

async fn save_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
) -> DraftResult<Option<ComposerDraftRevision>> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT revision FROM composer_draft_save_receipts WHERE request_id = ?",
            [Value::String(Some(request_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("read composer draft save receipt", source))?;
    row.map(|row| {
        parse_revision(
            row.try_get_by_index::<i64>(0).map_err(|source| {
                corrupt_data("composer_draft_save_receipts", "revision", source)
            })?,
        )
    })
    .transpose()
}

async fn record_save_receipt(
    transaction: &DatabaseTransaction,
    input: &SaveComposerDraftInput,
    revision: ComposerDraftRevision,
) -> DraftResult<()> {
    let saved_at = input.saved_at.as_millis();
    execute(
        transaction,
        "INSERT INTO composer_draft_save_receipts (request_id, scope_kind, scope_id, revision, saved_at_ms) VALUES (?, ?, ?, ?, ?)",
        [Value::String(Some(input.request_id.as_str().to_owned()))]
            .into_iter()
            .chain(scope_values(&input.scope))
            .chain([
                Value::BigInt(Some(revision.as_i64())),
                Value::BigInt(Some(saved_at)),
            ]),
        "record composer draft save receipt",
    )
    .await?;
    execute(
        transaction,
        "DELETE FROM composer_draft_save_receipts WHERE saved_at_ms < ?",
        [Value::BigInt(Some(saved_at.saturating_sub(
            COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS,
        )))],
        "prune composer draft save receipts",
    )
    .await
}

async fn execute(
    database: &impl ConnectionTrait,
    sql: &'static str,
    values: impl IntoIterator<Item = Value>,
    operation: &'static str,
) -> DraftResult<()> {
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .await
        .map(|_| ())
        .map_err(|source| database_error(operation, source))
}

fn scope_values(scope: &ComposerDraftScope) -> [Value; 2] {
    [
        Value::String(Some(scope.kind().to_owned())),
        Value::String(Some(scope.id().to_owned())),
    ]
}

pub(super) async fn ensure_scope_exists(
    database: &impl ConnectionTrait,
    scope: &ComposerDraftScope,
) -> DraftResult<()> {
    let sql = match scope {
        ComposerDraftScope::Thread(_) => "SELECT 1 FROM threads WHERE thread_id = ?",
        ComposerDraftScope::Project(_) => "SELECT 1 FROM attached_projects WHERE project_id = ?",
    };
    let found = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            [Value::String(Some(scope.id().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("find composer draft scope", source))?;
    if found.is_none() {
        return Err(ComposerDraftRepositoryError::ScopeNotFound {
            kind: scope.kind(),
            id: scope.id().to_owned(),
        });
    }
    Ok(())
}

async fn read_revision(
    database: &impl ConnectionTrait,
    scope: &ComposerDraftScope,
) -> DraftResult<Option<ComposerDraftRevision>> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            DRAFT_SELECT,
            scope_values(scope),
        ))
        .await
        .map_err(|source| database_error("read composer draft revision", source))?;
    row.map(|row| {
        parse_revision(
            row.try_get_by_index::<i64>(0)
                .map_err(|source| corrupt_data("composer_drafts", "revision", source))?,
        )
    })
    .transpose()
}

async fn verify_reference(
    database: &impl ConnectionTrait,
    reference: &ComposerAttachmentRef,
) -> DraftResult<()> {
    let digest = *reference.digest();
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            ATTACHMENT_META_SELECT,
            [Value::Bytes(Some(digest.as_bytes().to_vec()))],
        ))
        .await
        .map_err(|source| database_error("verify composer attachment", source))?
        .ok_or(ComposerDraftRepositoryError::AttachmentNotStored { digest })?;
    let mime_type = row
        .try_get_by_index::<String>(0)
        .map_err(|source| corrupt_data("composer_attachments", "mime_type", source))?;
    let size_bytes = row
        .try_get_by_index::<i64>(1)
        .map_err(|source| corrupt_data("composer_attachments", "size_bytes", source))?;
    if mime_type != reference.mime_type().as_str()
        || size_bytes != i64::from(reference.size_bytes())
    {
        return Err(ComposerDraftRepositoryError::AttachmentMismatch { digest });
    }
    Ok(())
}

pub(super) async fn read_draft(
    database: &impl ConnectionTrait,
    scope: &ComposerDraftScope,
) -> DraftResult<Option<ComposerDraft>> {
    let Some(row) = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            DRAFT_SELECT,
            scope_values(scope),
        ))
        .await
        .map_err(|source| database_error("read composer draft", source))?
    else {
        return Ok(None);
    };
    let revision = parse_revision(
        row.try_get_by_index::<i64>(0)
            .map_err(|source| corrupt_data("composer_drafts", "revision", source))?,
    )?;
    let text = AuthoredText::parse(
        row.try_get_by_index::<String>(1)
            .map_err(|source| corrupt_data("composer_drafts", "body", source))?,
    )
    .map_err(|source| corrupt_data("composer_drafts", "body", source))?;
    let updated_at = UnixMillis::from_millis(
        row.try_get_by_index::<i64>(2)
            .map_err(|source| corrupt_data("composer_drafts", "updated_at_ms", source))?,
    );
    let rows = database
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            DRAFT_ATTACHMENTS_SELECT,
            scope_values(scope),
        ))
        .await
        .map_err(|source| database_error("read composer draft attachments", source))?;
    let attachments = rows
        .iter()
        .map(|row| {
            let table = "composer_draft_attachments";
            let digest = ComposerAttachmentDigest::from_slice(
                &row.try_get_by_index::<Vec<u8>>(0)
                    .map_err(|source| corrupt_data(table, "digest", source))?,
            )
            .map_err(|source| corrupt_data(table, "digest", source))?;
            let name = row
                .try_get_by_index::<String>(1)
                .map_err(|source| corrupt_data(table, "name", source))?;
            let mime_type = parse_mime(
                &row.try_get_by_index::<String>(2)
                    .map_err(|source| corrupt_data(table, "mime_type", source))?,
            )?;
            let size_bytes = u32::try_from(
                row.try_get_by_index::<i64>(3)
                    .map_err(|source| corrupt_data(table, "size_bytes", source))?,
            )
            .map_err(|source| corrupt_data(table, "size_bytes", source))?;
            ComposerAttachmentRef::new(digest, mime_type, name, size_bytes)
                .map_err(|source| corrupt_data(table, "name", source))
        })
        .collect::<DraftResult<Vec<_>>>()?;
    ComposerDraft::new(revision, text, attachments, updated_at)
        .map(Some)
        .map_err(|source| corrupt_data("composer_draft_attachments", "digest", source))
}

fn parse_revision(value: i64) -> DraftResult<ComposerDraftRevision> {
    u64::try_from(value)
        .ok()
        .and_then(|value| ComposerDraftRevision::new(value).ok())
        .ok_or_else(|| corrupt_data("composer_drafts", "revision", "revision out of range"))
}

fn parse_mime(value: &str) -> DraftResult<ImageMimeType> {
    ImageMimeType::parse(value)
        .map_err(|source| corrupt_data("composer_attachments", "mime_type", source))
}
