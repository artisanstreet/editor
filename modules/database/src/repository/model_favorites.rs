//! Atomic persistence for the globally scoped model-favorites preference.
//!
//! This repository stores stable catalog ids only. Catalog admission and
//! runnable-engine policy belong to the backend caller; the database owns
//! bounded durable state, revisions, and exact request-id replay.

use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, SqliteTransactionMode, Statement,
    TransactionOptions, TransactionTrait, Value,
};
use thiserror::Error;

use artisan_domain::{
    MODEL_FAVORITES_MAX_MODELS, MODEL_FAVORITES_MAX_SNAPSHOT_BYTES, ModelFavoriteId,
    ModelFavoritesRevision, ModelFavoritesSnapshot, ReceiptDisposition, RequestId, UnixMillis,
};

use super::{Repository, RepositoryFailure, corrupt_data, database_error};

const FAVORITES_STATE_SELECT: &str =
    "SELECT revision FROM model_favorites_state WHERE state_id = 1";
const FAVORITES_ROWS_SELECT: &str = "SELECT model_id, favorited_at_ms FROM model_favorites ORDER BY favorited_at_ms ASC, model_id ASC LIMIT ?";
const FAVORITE_RECEIPT_SELECT: &str = "SELECT model_id, favorite, result_revision, snapshot_json FROM model_favorite_receipts WHERE request_id = ?";

/// Input for one intended model-favorite state mutation.
///
/// `favorite` is the desired state, not a toggle. The request id is stable
/// across retries of the same logical command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetModelFavoriteInput {
    /// Client request identity used for exact idempotency.
    pub request_id: RequestId,
    /// Stable model id admitted by the backend's current catalog.
    pub model_id: ModelFavoriteId,
    /// Desired state: `true` to retain/add, `false` to remove.
    pub favorite: bool,
    /// Forge acceptance timestamp used for new favorite ordering.
    pub accepted_at: UnixMillis,
}

/// Durable receipt and resulting snapshot for one favorite mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetModelFavoriteResult {
    request_id: RequestId,
    disposition: ReceiptDisposition,
    snapshot: ModelFavoritesSnapshot,
}

impl SetModelFavoriteResult {
    /// Returns the request identity answered by this result.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns whether this request newly committed or replayed a prior
    /// commit.
    #[must_use]
    pub const fn disposition(&self) -> ReceiptDisposition {
        self.disposition
    }

    /// Returns the snapshot produced by this request.
    #[must_use]
    pub const fn snapshot(&self) -> &ModelFavoritesSnapshot {
        &self.snapshot
    }

    /// Returns the snapshot revision produced by this request.
    #[must_use]
    pub const fn revision(&self) -> ModelFavoritesRevision {
        self.snapshot.revision()
    }
}

/// Failure at the model-favorites persistence boundary.
#[derive(Debug, Error)]
pub enum ModelFavoritesRepositoryError {
    /// The request id was already used with another model id or desired
    /// boolean.
    #[error("model favorite request `{request_id}` conflicts with an existing payload")]
    IdempotencyConflict {
        /// Conflicting request identity.
        request_id: RequestId,
    },
    /// The singleton revision row is absent.
    #[error("model favorites state row is missing")]
    MissingState,
    /// Persisted data violates a domain or schema invariant.
    #[error("persisted model favorites data is corrupt in {table}.{field}: {reason}")]
    CorruptData {
        /// Table containing the invalid value.
        table: &'static str,
        /// Column or logical field containing the invalid value.
        field: &'static str,
        /// Stable explanation suitable for diagnostics.
        reason: String,
    },
    /// The requested set operation would exceed the favorite cardinality
    /// bound.
    #[error("model favorites would contain {count} models; the maximum is {maximum}")]
    CardinalityExceeded {
        /// Number of models that would be stored.
        count: usize,
        /// Maximum number of models allowed.
        maximum: usize,
    },
    /// The revision cannot advance within its SQLite representation.
    #[error("model favorites revision cannot advance from {current}")]
    RevisionExhausted {
        /// Current exhausted revision.
        current: ModelFavoritesRevision,
    },
    /// The canonical receipt snapshot exceeded its finite storage bound.
    #[error("model favorites receipt snapshot is {bytes} bytes; the maximum is {maximum}")]
    SnapshotTooLarge {
        /// Canonical JSON byte length.
        bytes: usize,
        /// Maximum canonical JSON byte length.
        maximum: usize,
    },
    /// A local repository invariant failed while applying a transaction.
    #[error("model favorites persistence invariant failed: {reason}")]
    Invariant {
        /// Stable invariant description.
        reason: &'static str,
    },
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

impl RepositoryFailure for ModelFavoritesRepositoryError {
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

impl Repository {
    /// Reads the current globally scoped favorites snapshot.
    ///
    /// Rows are ordered by their persisted favorite timestamp and then stable
    /// model id, matching the Electron settings query's deterministic order.
    ///
    /// # Errors
    ///
    /// Returns an error when the singleton state is absent, persisted rows are
    /// corrupt, or SQLite cannot complete the read.
    pub async fn read_model_favorites(
        &self,
    ) -> Result<ModelFavoritesSnapshot, ModelFavoritesRepositoryError> {
        read_model_favorites_from(&self.database).await
    }

    /// Looks up a favorite receipt without changing durable state.
    ///
    /// A stored request must match both the stable model id and intended
    /// boolean. The receipt's original snapshot is returned, even when a
    /// later request has changed current favorites.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict for a different payload, or a typed
    /// error when the receipt data or database read is invalid.
    pub async fn lookup_set_model_favorite(
        &self,
        request_id: &RequestId,
        model_id: &ModelFavoriteId,
        favorite: bool,
    ) -> Result<Option<SetModelFavoriteResult>, ModelFavoritesRepositoryError> {
        lookup_set_model_favorite_on(&self.database, request_id, model_id, favorite).await
    }

    /// Atomically applies an intended favorite state and records its receipt.
    ///
    /// The SQLite transaction begins in `IMMEDIATE` mode, so concurrent
    /// writers serialize before reading the singleton revision. A receipt and
    /// state mutation commit together; this method emits no notifications.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict, cardinality/revision error, corrupt
    /// data error, or database error. All rejected writes roll back before
    /// returning.
    pub async fn set_model_favorite(
        &self,
        input: SetModelFavoriteInput,
    ) -> Result<SetModelFavoriteResult, ModelFavoritesRepositoryError> {
        if let Some(duplicate) = lookup_set_model_favorite_on(
            &self.database,
            &input.request_id,
            &input.model_id,
            input.favorite,
        )
        .await?
        {
            return Ok(duplicate);
        }

        let transaction = self
            .database
            .begin_with_options(TransactionOptions {
                sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
                ..Default::default()
            })
            .await
            .map_err(|source| database_error("begin model-favorites transaction", source))?;

        match apply_set_model_favorite(&transaction, &input).await {
            Ok(TransactionOutcome::Accepted(result)) => {
                transaction.commit().await.map_err(|source| {
                    database_error("commit model-favorites transaction", source)
                })?;
                Ok(result)
            }
            Ok(TransactionOutcome::Duplicate(result)) => {
                transaction.rollback().await.map_err(|source| {
                    database_error("rollback duplicate model-favorites request", source)
                })?;
                Ok(result)
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    database_error("rollback model-favorites transaction", source)
                })?;
                Err(error)
            }
        }
    }
}

#[derive(Debug)]
enum TransactionOutcome {
    Accepted(SetModelFavoriteResult),
    Duplicate(SetModelFavoriteResult),
}

async fn apply_set_model_favorite(
    transaction: &DatabaseTransaction,
    input: &SetModelFavoriteInput,
) -> Result<TransactionOutcome, ModelFavoritesRepositoryError> {
    if let Some(duplicate) = lookup_set_model_favorite_on(
        transaction,
        &input.request_id,
        &input.model_id,
        input.favorite,
    )
    .await?
    {
        return Ok(TransactionOutcome::Duplicate(duplicate));
    }

    let current = read_model_favorites_from(transaction).await?;
    let already_has_desired_state = current.contains(&input.model_id) == input.favorite;
    let snapshot = if already_has_desired_state {
        current
    } else {
        let next_revision = current.revision().checked_next().map_err(|_| {
            ModelFavoritesRepositoryError::RevisionExhausted {
                current: current.revision(),
            }
        })?;

        if input.favorite {
            let count = current.model_ids().len() + 1;
            if count > MODEL_FAVORITES_MAX_MODELS {
                return Err(ModelFavoritesRepositoryError::CardinalityExceeded {
                    count,
                    maximum: MODEL_FAVORITES_MAX_MODELS,
                });
            }
            insert_favorite(transaction, input).await?;
        } else {
            let deleted = delete_favorite(transaction, &input.model_id).await?;
            if deleted != 1 {
                return Err(ModelFavoritesRepositoryError::Invariant {
                    reason: "favorite row disappeared between snapshot read and delete",
                });
            }
        }

        update_revision(
            transaction,
            current.revision(),
            next_revision,
            input.accepted_at,
        )
        .await?;
        read_model_favorites_at_revision(transaction, next_revision).await?
    };

    let snapshot_json = encode_snapshot(&snapshot)?;
    let inserted = insert_receipt(transaction, input, &snapshot, &snapshot_json).await?;
    if inserted != 1 {
        let duplicate = lookup_set_model_favorite_on(
            transaction,
            &input.request_id,
            &input.model_id,
            input.favorite,
        )
        .await?;
        return duplicate.map(TransactionOutcome::Duplicate).ok_or(
            ModelFavoritesRepositoryError::Invariant {
                reason: "favorite receipt insert was ignored without a receipt",
            },
        );
    }

    Ok(TransactionOutcome::Accepted(SetModelFavoriteResult {
        request_id: input.request_id.clone(),
        disposition: ReceiptDisposition::Accepted,
        snapshot,
    }))
}

async fn read_model_favorites_from(
    database: &impl ConnectionTrait,
) -> Result<ModelFavoritesSnapshot, ModelFavoritesRepositoryError> {
    let state = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            FAVORITES_STATE_SELECT,
            std::iter::empty::<Value>(),
        ))
        .await
        .map_err(|source| database_error("read model-favorites state", source))?
        .ok_or(ModelFavoritesRepositoryError::MissingState)?;
    let revision_raw = state.try_get_by_index::<i64>(0).map_err(|source| {
        corrupt_data(
            "model_favorites_state",
            "revision",
            format!("could not decode revision: {source}"),
        )
    })?;
    let revision = parse_revision(revision_raw, "model_favorites_state", "revision")?;
    read_model_favorites_at_revision(database, revision).await
}

async fn read_model_favorites_at_revision(
    database: &impl ConnectionTrait,
    revision: ModelFavoritesRevision,
) -> Result<ModelFavoritesSnapshot, ModelFavoritesRepositoryError> {
    let limit = i64::try_from(MODEL_FAVORITES_MAX_MODELS + 1).map_err(|_| {
        ModelFavoritesRepositoryError::Invariant {
            reason: "favorite cardinality bound does not fit SQLite limit",
        }
    })?;
    let rows = database
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            FAVORITES_ROWS_SELECT,
            [Value::BigInt(Some(limit))],
        ))
        .await
        .map_err(|source| database_error("read model-favorites rows", source))?;
    if rows.len() > MODEL_FAVORITES_MAX_MODELS {
        return Err(ModelFavoritesRepositoryError::CardinalityExceeded {
            count: rows.len(),
            maximum: MODEL_FAVORITES_MAX_MODELS,
        });
    }

    let mut model_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let raw_model_id = row.try_get_by_index::<String>(0).map_err(|source| {
            corrupt_data(
                "model_favorites",
                "model_id",
                format!("could not decode model id: {source}"),
            )
        })?;
        let model_id = ModelFavoriteId::parse(raw_model_id)
            .map_err(|source| corrupt_data("model_favorites", "model_id", source.to_string()))?;
        // Decode the timestamp even though ordering is delegated to SQLite;
        // a non-integer row is corrupt rather than silently ignored.
        row.try_get_by_index::<i64>(1).map_err(|source| {
            corrupt_data(
                "model_favorites",
                "favorited_at_ms",
                format!("could not decode timestamp: {source}"),
            )
        })?;
        model_ids.push(model_id);
    }

    ModelFavoritesSnapshot::new(revision, model_ids)
        .map_err(|source| corrupt_data("model_favorites", "model_id", source.to_string()))
}

async fn lookup_set_model_favorite_on(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    model_id: &ModelFavoriteId,
    favorite: bool,
) -> Result<Option<SetModelFavoriteResult>, ModelFavoritesRepositoryError> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            FAVORITE_RECEIPT_SELECT,
            [Value::String(Some(request_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("find model-favorite receipt", source))?;
    let Some(row) = row else {
        return Ok(None);
    };

    let stored_model_id = row.try_get_by_index::<String>(0).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "model_id",
            format!("could not decode model id: {source}"),
        )
    })?;
    let stored_favorite = row.try_get_by_index::<i64>(1).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "favorite",
            format!("could not decode boolean: {source}"),
        )
    })?;
    let stored_result_revision = row.try_get_by_index::<i64>(2).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "result_revision",
            format!("could not decode revision: {source}"),
        )
    })?;
    let snapshot_json = row.try_get_by_index::<String>(3).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            format!("could not decode snapshot: {source}"),
        )
    })?;

    let stored_model_id = ModelFavoriteId::parse(stored_model_id).map_err(|source| {
        corrupt_data("model_favorite_receipts", "model_id", source.to_string())
    })?;
    if !matches!(stored_favorite, 0 | 1) {
        return Err(corrupt_data(
            "model_favorite_receipts",
            "favorite",
            "persisted favorite must be zero or one",
        ));
    }
    if stored_model_id != *model_id {
        return Err(ModelFavoritesRepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }
    let stored_favorite = stored_favorite == 1;
    if stored_favorite != favorite {
        return Err(ModelFavoritesRepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }

    let revision = parse_revision(
        stored_result_revision,
        "model_favorite_receipts",
        "result_revision",
    )?;
    let snapshot = decode_snapshot(&snapshot_json, revision)?;
    Ok(Some(SetModelFavoriteResult {
        request_id: request_id.clone(),
        disposition: ReceiptDisposition::Duplicate,
        snapshot,
    }))
}

async fn insert_favorite(
    database: &impl ConnectionTrait,
    input: &SetModelFavoriteInput,
) -> Result<(), ModelFavoritesRepositoryError> {
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO model_favorites (model_id, favorited_at_ms) VALUES (?, ?)",
            [
                Value::String(Some(input.model_id.as_str().to_owned())),
                Value::BigInt(Some(input.accepted_at.as_millis())),
            ],
        ))
        .await
        .map(|_| ())
        .map_err(|source| database_error("insert model favorite", source))
}

async fn delete_favorite(
    database: &impl ConnectionTrait,
    model_id: &ModelFavoriteId,
) -> Result<u64, ModelFavoritesRepositoryError> {
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "DELETE FROM model_favorites WHERE model_id = ?",
            [Value::String(Some(model_id.as_str().to_owned()))],
        ))
        .await
        .map(|result| result.rows_affected())
        .map_err(|source| database_error("delete model favorite", source))
}

async fn update_revision(
    database: &impl ConnectionTrait,
    current: ModelFavoritesRevision,
    next: ModelFavoritesRevision,
    accepted_at: UnixMillis,
) -> Result<(), ModelFavoritesRepositoryError> {
    let updated = database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE model_favorites_state SET revision = ?, updated_at_ms = ? WHERE state_id = 1 AND revision = ?",
            [
                Value::BigInt(Some(next.as_i64())),
                Value::BigInt(Some(accepted_at.as_millis())),
                Value::BigInt(Some(current.as_i64())),
            ],
        ))
        .await
        .map_err(|source| database_error("advance model-favorites revision", source))?
        .rows_affected();
    if updated != 1 {
        return Err(ModelFavoritesRepositoryError::Invariant {
            reason: "model-favorites revision update did not affect singleton state",
        });
    }
    Ok(())
}

async fn insert_receipt(
    database: &impl ConnectionTrait,
    input: &SetModelFavoriteInput,
    snapshot: &ModelFavoritesSnapshot,
    snapshot_json: &str,
) -> Result<u64, ModelFavoritesRepositoryError> {
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT OR IGNORE INTO model_favorite_receipts (request_id, model_id, favorite, result_revision, snapshot_json, accepted_at_ms) VALUES (?, ?, ?, ?, ?, ?)",
            [
                Value::String(Some(input.request_id.as_str().to_owned())),
                Value::String(Some(input.model_id.as_str().to_owned())),
                Value::Bool(Some(input.favorite)),
                Value::BigInt(Some(snapshot.revision().as_i64())),
                Value::String(Some(snapshot_json.to_owned())),
                Value::BigInt(Some(input.accepted_at.as_millis())),
            ],
        ))
        .await
        .map(|result| result.rows_affected())
        .map_err(|source| database_error("record model-favorite receipt", source))
}

fn encode_snapshot(
    snapshot: &ModelFavoritesSnapshot,
) -> Result<String, ModelFavoritesRepositoryError> {
    let model_ids = snapshot
        .model_ids()
        .iter()
        .map(ModelFavoriteId::as_str)
        .collect::<Vec<_>>();
    let encoded = serde_json::to_string(&model_ids).map_err(|_| {
        ModelFavoritesRepositoryError::Invariant {
            reason: "model-favorites snapshot could not be JSON encoded",
        }
    })?;
    if encoded.len() > MODEL_FAVORITES_MAX_SNAPSHOT_BYTES {
        return Err(ModelFavoritesRepositoryError::SnapshotTooLarge {
            bytes: encoded.len(),
            maximum: MODEL_FAVORITES_MAX_SNAPSHOT_BYTES,
        });
    }
    Ok(encoded)
}

fn decode_snapshot(
    encoded: &str,
    revision: ModelFavoritesRevision,
) -> Result<ModelFavoritesSnapshot, ModelFavoritesRepositoryError> {
    if encoded.len() > MODEL_FAVORITES_MAX_SNAPSHOT_BYTES {
        return Err(corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            format!(
                "snapshot is {} bytes; maximum is {}",
                encoded.len(),
                MODEL_FAVORITES_MAX_SNAPSHOT_BYTES
            ),
        ));
    }
    let raw_model_ids = serde_json::from_str::<Vec<String>>(encoded).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            format!("invalid JSON array: {source}"),
        )
    })?;
    let canonical = serde_json::to_string(&raw_model_ids).map_err(|_| {
        corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            "snapshot could not be canonically encoded".to_owned(),
        )
    })?;
    if canonical != encoded {
        return Err(corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            "snapshot JSON is not canonical",
        ));
    }
    let model_ids = raw_model_ids
        .into_iter()
        .map(|raw_model_id| {
            ModelFavoriteId::parse(raw_model_id).map_err(|source| {
                corrupt_data(
                    "model_favorite_receipts",
                    "snapshot_json",
                    source.to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    ModelFavoritesSnapshot::new(revision, model_ids).map_err(|source| {
        corrupt_data(
            "model_favorite_receipts",
            "snapshot_json",
            source.to_string(),
        )
    })
}

fn parse_revision(
    value: i64,
    table: &'static str,
    field: &'static str,
) -> Result<ModelFavoritesRevision, ModelFavoritesRepositoryError> {
    let value = u64::try_from(value)
        .map_err(|_| corrupt_data(table, field, "revision must be non-negative".to_owned()))?;
    ModelFavoritesRevision::new(value)
        .map_err(|source| corrupt_data(table, field, source.to_string()))
}
