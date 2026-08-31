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

        let thread = entities::thread::Entity::find_by_id(input.thread_id.as_str())
            .one(&transaction)
            .await
            .map_err(|source| database_error("find engine-config thread", source))?
            .ok_or_else(|| RepositoryError::ThreadNotFound {
                thread_id: input.thread_id.clone(),
            })?;
        let current = settings_from_thread(thread.clone())?;
        let current_revision = current.as_ref().map(ThreadEngineSettings::revision);
        let expected_revision = input.precondition.expected_revision();
        if current_revision != expected_revision {
            return rollback_with_error(
                transaction,
                RepositoryError::EngineConfigRevisionConflict {
                    thread_id: input.thread_id,
                    expected_revision,
                    actual_revision: current_revision,
                },
            )
            .await;
        }

        let next_revision = match current_revision {
            None => EngineConfigRevision::new(1)
                .map_err(|error| corrupt_data("threads", "engine_run_config_revision", &error))?,
            Some(revision) => revision
                .checked_next()
                .map_err(|error| corrupt_data("threads", "engine_run_config_revision", &error))?,
        };
        let previous_revision = thread.engine_run_config_revision;
        let previous_version = thread.engine_run_config_version;
        let previous_blob = thread
            .engine_run_config
            .as_ref()
            .map(|blob| blob.as_slice().to_vec());
        let updated_at_ms = thread.updated_at_ms.max(millis(input.accepted_at));
        let update = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE threads SET engine_run_config_version = 1, engine_run_config_revision = ?, engine_run_config = ?, updated_at_ms = ? WHERE thread_id = ? AND engine_run_config_version IS ? AND engine_run_config_revision = ? AND engine_run_config IS ?",
            [
                Value::BigInt(Some(next_revision.as_i64())),
                Value::Bytes(Some(Box::new(encoded.clone()))),
                Value::BigInt(Some(updated_at_ms)),
                Value::String(Some(input.thread_id.as_str().to_owned())),
                optional_i64(previous_version),
                Value::BigInt(Some(previous_revision)),
                optional_bytes(previous_blob),
            ],
        );
        let updated = transaction
            .execute(update)
            .await
            .map_err(|source| database_error("update thread engine configuration", source))?;
        if updated.rows_affected() != 1 {
            let current = entities::thread::Entity::find_by_id(input.thread_id.as_str())
                .one(&transaction)
                .await
                .map_err(|source| database_error("recheck engine-config thread", source))?
                .ok_or_else(|| RepositoryError::ThreadNotFound {
                    thread_id: input.thread_id.clone(),
                })?;
            let actual_revision = settings_from_thread(current)?.map(|settings| settings.revision);
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

        let receipt_inserted =
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
                engine_run_config: Set(Some(OpaqueBytes::new(encoded))),
                engine_run_config_expected_revision: Set(
                    expected_revision.map(EngineConfigRevision::as_i64)
                ),
                engine_run_config_result_revision: Set(Some(next_revision.as_i64())),
            })
            .on_conflict(do_nothing_on_conflict())
            .exec_without_returning(&transaction)
            .await
            .map_err(|source| database_error("record engine-config receipt", source))?;
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

async fn lookup_set_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    thread_id: &ThreadId,
    precondition: EngineConfigUpdatePrecondition,
    config: &EngineRunConfig,
    encoded: &[u8],
) -> Result<Option<SetThreadEngineConfigResult>, RepositoryError> {
    let Some(row) = entities::command_receipt::Entity::find_by_id(request_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find engine-config receipt", source))?
    else {
        return Ok(None);
    };
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
    let persisted_expected_revision = match row.engine_run_config_expected_revision {
        None => None,
        Some(value) => Some(
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
        ),
    };
    let stored_blob = row.engine_run_config.as_ref().ok_or_else(|| {
        corrupt_data(
            "command_receipts",
            "engine_run_config",
            "required value is null",
        )
    })?;
    let stored_config = engine_run_config::decode(stored_blob.as_slice())
        .map_err(|error| corrupt_data("command_receipts", "engine_run_config", &error))?;
    let result_revision = row
        .engine_run_config_result_revision
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
    Value::Bytes(value.map(Box::new))
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
