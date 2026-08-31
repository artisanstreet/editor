//! Atomic persistence for durable thread engine configuration.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ConnectionTrait, DbBackend, EntityTrait, Statement, TransactionTrait, Value,
};

use artisan_domain::{
    CommandReceipt, EngineConfigRevision, EngineConfigUpdatePrecondition, EngineRunConfig,
    RequestId, ThreadId, UnixMillis,
};

use crate::engine_run_config::{self, EngineRunConfigCodecError};
use crate::entities::{self, CommandKind, OpaqueBytes};

use super::{Repository, RepositoryError, corrupt_data, database_error, millis};

/// Immutable settings read from a configured thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadEngineSettings {
    revision: EngineConfigRevision,
    config: EngineRunConfig,
}

impl ThreadEngineSettings {
    fn new(revision: EngineConfigRevision, config: EngineRunConfig) -> Self {
        Self { revision, config }
    }

    /// Returns the thread configuration revision.
    #[must_use]
    pub const fn revision(&self) -> EngineConfigRevision {
        self.revision
    }

    /// Returns the immutable engine configuration.
    #[must_use]
    pub const fn config(&self) -> &EngineRunConfig {
        &self.config
    }
}

/// Inputs for one authenticated thread configuration mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetThreadEngineConfigInput {
    pub request_id: RequestId,
    pub thread_id: ThreadId,
    pub precondition: EngineConfigUpdatePrecondition,
    pub config: EngineRunConfig,
    pub accepted_at: UnixMillis,
}

/// Durable receipt and resulting revision for one configuration mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetThreadEngineConfigResult {
    receipt: CommandReceipt,
    thread_id: ThreadId,
    revision: EngineConfigRevision,
}

impl SetThreadEngineConfigResult {
    /// Returns the command receipt disposition and request identity.
    #[must_use]
    pub const fn receipt(&self) -> &CommandReceipt {
        &self.receipt
    }

    /// Returns the configured thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the resulting configuration revision.
    #[must_use]
    pub const fn revision(&self) -> EngineConfigRevision {
        self.revision
    }
}

impl Repository {
    /// Looks up a configuration receipt without consulting a clock or
    /// changing any durable state.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration cannot be canonically encoded,
    /// the receipt lookup fails, or the stored receipt is corrupt or conflicts
    /// with the supplied request.
    pub async fn lookup_set_thread_engine_config(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        precondition: EngineConfigUpdatePrecondition,
        config: &EngineRunConfig,
    ) -> Result<Option<SetThreadEngineConfigResult>, RepositoryError> {
        let encoded = encode_config(config)?;
        lookup_set_receipt(
            &self.database,
            request_id,
            thread_id,
            precondition,
            config,
            &encoded,
        )
        .await
    }

    /// Atomically updates a thread's configuration and records its receipt.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration encoding or persistence fails, the
    /// thread or receipt data is missing or corrupt, the request conflicts with
    /// a stored receipt, or its revision precondition is stale.
    pub async fn set_thread_engine_config(
        &self,
        input: SetThreadEngineConfigInput,
    ) -> Result<SetThreadEngineConfigResult, RepositoryError> {
        let encoded = encode_config(&input.config)?;
        if let Some(duplicate) = lookup_set_receipt(
            &self.database,
            &input.request_id,
            &input.thread_id,
            input.precondition,
            &input.config,
            &encoded,
        )
        .await?
        {
            return Ok(duplicate);
        }

        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin engine-config transaction", source))?;

        if let Some(duplicate) = lookup_set_receipt(
            &transaction,
            &input.request_id,
            &input.thread_id,
            input.precondition,
            &input.config,
            &encoded,
        )
        .await?
        {
            transaction.rollback().await.map_err(|source| {
                database_error("finish duplicate engine-config request", source)
            })?;
            return Ok(duplicate);
        }

        let thread =
            load_engine_config_thread(&transaction, &input.thread_id, "find engine-config thread")
                .await?;
        let expected_revision = input.precondition.expected_revision();
        let next_revision =
            match next_revision_for_update(&thread, &input.thread_id, expected_revision) {
                Ok(next_revision) => next_revision,
                Err(error) => return rollback_with_error(transaction, error).await,
            };

        if update_thread_engine_config(&transaction, &thread, &input, &encoded, next_revision)
            .await?
            != 1
        {
            let actual_revision = rechecked_thread_revision(&transaction, &input.thread_id).await?;
            return rollback_with_error(
                transaction,
                RepositoryError::EngineConfigRevisionConflict {
                    thread_id: input.thread_id,
                    expected_revision,
                    actual_revision,
                },
            )
            .await;
        }

        let receipt_inserted = insert_set_receipt(
            &transaction,
            &input,
            &encoded,
            expected_revision,
            next_revision,
        )
        .await?;
        if receipt_inserted != 1 {
            let duplicate = lookup_set_receipt(
                &transaction,
                &input.request_id,
                &input.thread_id,
                input.precondition,
                &input.config,
                &encoded,
            )
            .await;
            return rollback_with_result(transaction, duplicate).await;
        }

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit engine-config transaction", source))?;
        Ok(set_result(
            input.request_id,
            input.thread_id,
            next_revision,
            artisan_domain::ReceiptDisposition::Accepted,
        ))
    }

    /// Reads a configured thread's immutable settings. The exact null/zero
    /// sentinel is the only unconfigured result.
    ///
    /// # Errors
    ///
    /// Returns an error if the database read fails, the thread does not exist,
    /// or its stored configuration is corrupt.
    pub async fn read_thread_engine_settings(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Option<ThreadEngineSettings>, RepositoryError> {
        let thread = entities::thread::Entity::find_by_id(thread_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read thread engine configuration", source))?
            .ok_or_else(|| RepositoryError::ThreadNotFound {
                thread_id: thread_id.clone(),
            })?;
        settings_from_thread(thread)
    }
}

fn encode_config(config: &EngineRunConfig) -> Result<Vec<u8>, RepositoryError> {
    engine_run_config::encode(config).map_err(|error| match error {
        EngineRunConfigCodecError::InvalidField { field } => {
            corrupt_data("engine_run_config", field, "invalid configuration")
        }
        EngineRunConfigCodecError::TooLarge => {
            corrupt_data("engine_run_config", "blob", "encoded value exceeds bound")
        }
        EngineRunConfigCodecError::Malformed
        | EngineRunConfigCodecError::NonCanonical
        | EngineRunConfigCodecError::Encode => corrupt_data(
            "engine_run_config",
            "blob",
            "could not encode configuration",
        ),
    })
}

pub(super) fn settings_from_thread(
    thread: entities::Thread,
) -> Result<Option<ThreadEngineSettings>, RepositoryError> {
    match (
        thread.engine_run_config_version,
        thread.engine_run_config_revision,
        thread.engine_run_config,
    ) {
        (None, 0, None) => Ok(None),
        (Some(1), revision, Some(blob)) => {
            let revision = u64::try_from(revision)
                .ok()
                .and_then(|value| EngineConfigRevision::new(value).ok())
                .ok_or_else(|| {
                    corrupt_data(
                        "threads",
                        "engine_run_config_revision",
                        "revision is outside its domain range",
                    )
                })?;
            let config = engine_run_config::decode(blob.as_slice())
                .map_err(|error| corrupt_data("threads", "engine_run_config", &error))?;
            Ok(Some(ThreadEngineSettings::new(revision, config)))
        }
        _ => Err(corrupt_data(
            "threads",
            "engine_run_config",
            "configuration tuple is not a valid sentinel or snapshot",
        )),
    }
}

async fn load_engine_config_thread(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
    operation: &'static str,
) -> Result<entities::Thread, RepositoryError> {
    entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error(operation, source))?
        .ok_or_else(|| RepositoryError::ThreadNotFound {
            thread_id: thread_id.clone(),
        })
}

fn next_revision_for_update(
    thread: &entities::Thread,
    thread_id: &ThreadId,
    expected_revision: Option<EngineConfigRevision>,
) -> Result<EngineConfigRevision, RepositoryError> {
    let current = settings_from_thread(thread.clone())?;
    let current_revision = current.as_ref().map(ThreadEngineSettings::revision);
    if current_revision != expected_revision {
        return Err(RepositoryError::EngineConfigRevisionConflict {
            thread_id: thread_id.clone(),
            expected_revision,
            actual_revision: current_revision,
        });
    }

    match current_revision {
        None => EngineConfigRevision::new(1)
            .map_err(|error| corrupt_data("threads", "engine_run_config_revision", &error)),
        Some(revision) => revision
            .checked_next()
            .map_err(|error| corrupt_data("threads", "engine_run_config_revision", &error)),
    }
}

async fn update_thread_engine_config(
    transaction: &sea_orm::DatabaseTransaction,
    thread: &entities::Thread,
    input: &SetThreadEngineConfigInput,
    encoded: &[u8],
    next_revision: EngineConfigRevision,
) -> Result<u64, RepositoryError> {
    let previous_revision = thread.engine_run_config_revision;
    let previous_version = thread.engine_run_config_version;
    let previous_blob = thread
        .engine_run_config
        .as_ref()
        .map(|blob| blob.as_slice().to_vec());
    let update = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE threads SET engine_run_config_version = 1, engine_run_config_revision = ?, engine_run_config = ?, updated_at_ms = MAX(updated_at_ms, ?) WHERE thread_id = ? AND engine_run_config_version IS ? AND engine_run_config_revision = ? AND engine_run_config IS ?",
        [
            Value::BigInt(Some(next_revision.as_i64())),
            Value::Bytes(Some(encoded.to_vec())),
            Value::BigInt(Some(millis(input.accepted_at))),
            Value::String(Some(input.thread_id.as_str().to_owned())),
            optional_i64(previous_version),
            Value::BigInt(Some(previous_revision)),
            optional_bytes(previous_blob),
        ],
    );
    let updated = transaction
        .execute_raw(update)
        .await
        .map_err(|source| database_error("update thread engine configuration", source))?;
    Ok(updated.rows_affected())
}

async fn rechecked_thread_revision(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &ThreadId,
) -> Result<Option<EngineConfigRevision>, RepositoryError> {
    let current =
        load_engine_config_thread(transaction, thread_id, "recheck engine-config thread").await?;
    Ok(settings_from_thread(current)?.map(|settings| settings.revision))
}

async fn insert_set_receipt(
    transaction: &sea_orm::DatabaseTransaction,
    input: &SetThreadEngineConfigInput,
    encoded: &[u8],
    expected_revision: Option<EngineConfigRevision>,
    next_revision: EngineConfigRevision,
) -> Result<u64, RepositoryError> {
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set(input.request_id.as_str().to_owned()),
        command_kind: Set(CommandKind::SetThreadEngineConfig),
        directory_id: Set(None),
        project_id: Set(None),
        thread_id: Set(Some(input.thread_id.as_str().to_owned())),
        title: Set(None),
        message_id: Set(None),
        body: Set(None),
        accepted_at_ms: Set(millis(input.accepted_at)),
        engine_run_config_version: Set(Some(1)),
        engine_run_config: Set(Some(OpaqueBytes::new(encoded.to_vec()))),
        engine_run_config_expected_revision: Set(
            expected_revision.map(EngineConfigRevision::as_i64)
        ),
        engine_run_config_result_revision: Set(Some(next_revision.as_i64())),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(transaction)
    .await
    .map_err(|source| database_error("record engine-config receipt", source))
}

async fn receipt_row_by_id(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
) -> Result<Option<entities::CommandReceipt>, RepositoryError> {
    entities::command_receipt::Entity::find_by_id(request_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find engine-config receipt", source))
}

fn validate_set_receipt_shape<'a>(
    row: &'a entities::CommandReceipt,
    request_id: &RequestId,
) -> Result<&'a str, RepositoryError> {
    if row.command_kind != CommandKind::SetThreadEngineConfig {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }
    if row.directory_id.is_some()
        || row.project_id.is_some()
        || row.title.is_some()
        || row.message_id.is_some()
        || row.body.is_some()
    {
        return Err(corrupt_data(
            "command_receipts",
            "engine_run_config",
            "set receipt has an invalid legacy payload shape",
        ));
    }
    let persisted_thread_id = row.thread_id.as_deref().ok_or_else(|| {
        corrupt_data(
            "command_receipts",
            "thread_id",
            "set receipt thread is null",
        )
    })?;
    if row.engine_run_config_version != Some(1) {
        return Err(corrupt_data(
            "command_receipts",
            "engine_run_config_version",
            "set receipt must use version one",
        ));
    }
    Ok(persisted_thread_id)
}

fn parse_set_receipt_expected_revision(
    value: Option<i64>,
) -> Result<Option<EngineConfigRevision>, RepositoryError> {
    match value {
        None => Ok(None),
        Some(value) => Ok(Some(
            EngineConfigRevision::new(u64::try_from(value).map_err(|_| {
                corrupt_data(
                    "command_receipts",
                    "engine_run_config_expected_revision",
                    "expected revision is outside its domain range",
                )
            })?)
            .map_err(|error| {
                corrupt_data(
                    "command_receipts",
                    "engine_run_config_expected_revision",
                    &error,
                )
            })?,
        )),
    }
}

fn validate_set_receipt_result_revision(
    value: Option<i64>,
    persisted_expected_revision: Option<EngineConfigRevision>,
) -> Result<EngineConfigRevision, RepositoryError> {
    let result_revision = value
        .and_then(|value| u64::try_from(value).ok())
        .and_then(|value| EngineConfigRevision::new(value).ok())
        .ok_or_else(|| {
            corrupt_data(
                "command_receipts",
                "engine_run_config_result_revision",
                "result revision is outside its domain range",
            )
        })?;
    let expected_result_revision = match persisted_expected_revision {
        None => 1,
        Some(revision) => revision
            .checked_next()
            .map_err(|error| {
                corrupt_data(
                    "command_receipts",
                    "engine_run_config_result_revision",
                    &error,
                )
            })?
            .get(),
    };
    if result_revision.get() != expected_result_revision {
        return Err(corrupt_data(
            "command_receipts",
            "engine_run_config_result_revision",
            "result revision does not follow its precondition",
        ));
    }
    Ok(result_revision)
}

async fn lookup_set_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    thread_id: &ThreadId,
    precondition: EngineConfigUpdatePrecondition,
    config: &EngineRunConfig,
    encoded: &[u8],
) -> Result<Option<SetThreadEngineConfigResult>, RepositoryError> {
    let Some(row) = receipt_row_by_id(database, request_id).await? else {
        return Ok(None);
    };
    let persisted_thread_id = validate_set_receipt_shape(&row, request_id)?;
    let persisted_expected_revision =
        parse_set_receipt_expected_revision(row.engine_run_config_expected_revision)?;
    let stored_blob = row.engine_run_config.as_ref().ok_or_else(|| {
        corrupt_data(
            "command_receipts",
            "engine_run_config",
            "required value is null",
        )
    })?;
    let stored_config = engine_run_config::decode(stored_blob.as_slice())
        .map_err(|error| corrupt_data("command_receipts", "engine_run_config", &error))?;
    let result_revision = validate_set_receipt_result_revision(
        row.engine_run_config_result_revision,
        persisted_expected_revision,
    )?;
    if persisted_thread_id != thread_id.as_str()
        || persisted_expected_revision != precondition.expected_revision()
        || stored_blob.as_slice() != encoded
        || stored_config != *config
    {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }
    Ok(Some(set_result(
        request_id.clone(),
        thread_id.clone(),
        result_revision,
        artisan_domain::ReceiptDisposition::Duplicate,
    )))
}

fn set_result(
    request_id: RequestId,
    thread_id: ThreadId,
    revision: EngineConfigRevision,
    disposition: artisan_domain::ReceiptDisposition,
) -> SetThreadEngineConfigResult {
    SetThreadEngineConfigResult {
        receipt: CommandReceipt {
            request_id,
            disposition,
        },
        thread_id,
        revision,
    }
}

fn optional_i64(value: Option<i64>) -> Value {
    Value::BigInt(value)
}

fn optional_bytes(value: Option<Vec<u8>>) -> Value {
    Value::Bytes(value)
}

fn do_nothing_on_conflict() -> OnConflict {
    let mut conflict = OnConflict::new();
    conflict.do_nothing();
    conflict
}

async fn rollback_with_error<T>(
    transaction: sea_orm::DatabaseTransaction,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("rollback engine-config transaction", source))?;
    Err(error)
}

async fn rollback_with_result(
    transaction: sea_orm::DatabaseTransaction,
    result: Result<Option<SetThreadEngineConfigResult>, RepositoryError>,
) -> Result<SetThreadEngineConfigResult, RepositoryError> {
    let result = result;
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("finish engine-config receipt race", source))?;
    match result? {
        Some(result) => Ok(result),
        None => Err(RepositoryError::Invariant {
            reason: "engine-config receipt insert was ignored without an identifiable receipt",
        }),
    }
}
