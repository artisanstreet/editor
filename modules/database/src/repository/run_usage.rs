//! Transactional current provider-usage persistence for one assistant run.
//!
//! The repository keeps one bounded report per run. It verifies the run's
//! immutable engine snapshot before every read/write, then applies provider
//! session and source-sequence fencing in the same SQLite transaction. No raw
//! provider envelope is accepted or stored here.

use artisan_domain::{
    EngineConfigRevision, EngineModelId, EngineRouteId, EngineSelection, EngineVariantId, RunId,
    RunUsageBasis, RunUsageReport, RunUsageReportInput, ThreadId, UnixMillis,
};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, TransactionTrait, Value};
use thiserror::Error;

use crate::engine_run_config;
use crate::entities;

use super::{Repository, RepositoryError, database_error};

const SELECT_USAGE_SQL: &str = "SELECT thread_id, generation, provider_session_id, source_sequence, basis, provider_turn_id, model_id, provider_route_id, variant_id, input_tokens, cached_input_tokens, output_tokens, context_tokens, context_window_tokens, observed_at_ms FROM run_usage WHERE run_id = ?";

const INSERT_USAGE_SQL: &str = "INSERT INTO run_usage (run_id, thread_id, generation, provider_session_id, source_sequence, basis, provider_turn_id, model_id, provider_route_id, variant_id, input_tokens, cached_input_tokens, output_tokens, context_tokens, context_window_tokens, observed_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(run_id) DO NOTHING";

const UPDATE_USAGE_SQL: &str = "UPDATE run_usage SET provider_session_id = ?, source_sequence = ?, basis = ?, provider_turn_id = ?, model_id = ?, provider_route_id = ?, variant_id = ?, input_tokens = ?, cached_input_tokens = ?, output_tokens = ?, context_tokens = ?, context_window_tokens = ?, observed_at_ms = ? WHERE run_id = ? AND thread_id = ? AND generation = ? AND provider_session_id = ? AND source_sequence = ? AND source_sequence < ?";

/// Borrowed input for one provider usage write.
pub struct RecordRunUsage<'a> {
    /// Explicit run scope supplied by the owner.
    pub run_id: &'a RunId,
    /// Explicit thread scope supplied by the owner.
    pub thread_id: &'a ThreadId,
    /// Parsed provider report to persist.
    pub report: &'a RunUsageReport,
}

/// Descriptive alias for callers that use `Input` naming.
pub type RecordRunUsageInput<'a> = RecordRunUsage<'a>;

/// Nonsecret receipt information for a usage write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunUsageWriteReceipt {
    run_id: RunId,
    thread_id: ThreadId,
    generation: i64,
    source_sequence: u64,
}

impl RunUsageWriteReceipt {
    /// Returns the run scope.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the thread scope.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the persisted run generation.
    #[must_use]
    pub const fn generation(&self) -> i64 {
        self.generation
    }

    /// Returns the accepted provider source sequence.
    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }
}

/// Classification of one transactionally processed report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordRunUsageOutcome {
    /// No usage existed for the run and this report was inserted.
    Recorded(RunUsageWriteReceipt),
    /// A higher provider sequence replaced the current report.
    Replaced(RunUsageWriteReceipt),
    /// The same provider sequence and measurement were already persisted.
    Duplicate(RunUsageWriteReceipt),
}

/// Typed failures at the run-usage persistence boundary.
#[derive(Debug, Error)]
pub enum RunUsageRepositoryError {
    #[error("run usage report run identity does not match the command")]
    ReportRunMismatch,
    #[error("run usage report thread identity does not match the command")]
    ReportThreadMismatch,
    #[error("assistant run `{run_id}` does not exist")]
    RunNotFound { run_id: RunId },
    #[error("assistant run `{run_id}` is not associated with the requested thread")]
    RunThreadMismatch { run_id: RunId, thread_id: ThreadId },
    #[error("assistant run `{run_id}` has an invalid immutable engine snapshot")]
    InvalidRunSnapshot { run_id: RunId },
    #[error("usage model origin does not match the immutable run snapshot")]
    ModelOriginMismatch { run_id: RunId },
    #[error("stored usage row `{run_id}` belongs to a different thread")]
    StoredThreadMismatch { run_id: RunId },
    #[error("stored usage row `{run_id}` belongs to a different run generation")]
    GenerationMismatch { run_id: RunId },
    #[error("provider session conflicts with the usage already stored for run `{run_id}`")]
    ProviderSessionConflict { run_id: RunId },
    #[error("provider source sequence {incoming} is stale behind {current} for run `{run_id}`")]
    StaleSequence {
        run_id: RunId,
        current: u64,
        incoming: u64,
    },
    #[error("provider source sequence {sequence} has conflicting usage for run `{run_id}`")]
    SequenceConflict { run_id: RunId, sequence: u64 },
    #[error("stored usage row `{run_id}` is corrupt")]
    CorruptUsageRow { run_id: RunId },
    #[error("database repository operation failed")]
    Repository(#[source] RepositoryError),
}

struct RunUsageAuthority {
    generation: i64,
    model_id: EngineModelId,
    provider_route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
}

struct StoredUsage {
    generation: i64,
    report: RunUsageReport,
}

impl Repository {
    /// Records or monotonically replaces the current usage for one run.
    ///
    /// The explicit command scope must match the report. The persisted run
    /// snapshot is then checked before the row is inserted or replaced.
    ///
    /// # Errors
    ///
    /// Returns [`RunUsageRepositoryError`] when the command scope disagrees
    /// with the report, the run snapshot does not match, a stored usage row
    /// fails validation, or a transaction fails.
    pub async fn record_run_usage(
        &self,
        command: RecordRunUsage<'_>,
    ) -> Result<RecordRunUsageOutcome, RunUsageRepositoryError> {
        validate_command_scope(&command)?;
        let transaction = self.database.begin().await.map_err(|source| {
            repository_error(database_error("begin run usage transaction", source))
        })?;

        let authority =
            match load_run_authority(&transaction, command.run_id, command.thread_id).await {
                Ok(authority) => authority,
                Err(error) => return rollback_with_error(transaction, error).await,
            };
        if !report_matches_authority(&authority, command.report) {
            return rollback_with_error(
                transaction,
                RunUsageRepositoryError::ModelOriginMismatch {
                    run_id: command.run_id.clone(),
                },
            )
            .await;
        }

        let result = record_in_transaction(&transaction, &authority, &command).await;
        match result {
            Ok(result) => {
                transaction.commit().await.map_err(|source| {
                    repository_error(database_error("commit run usage", source))
                })?;
                Ok(result)
            }
            Err(error) => rollback_with_error(transaction, error).await,
        }
    }

    /// Reads the current (highest accepted provider sequence) usage for an
    /// exact run/thread scope after revalidating its immutable run snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`RunUsageRepositoryError`] when the run snapshot is missing
    /// or invalid, a stored usage row fails validation, or a transaction
    /// fails.
    pub async fn read_latest_run_usage(
        &self,
        run_id: &RunId,
        thread_id: &ThreadId,
    ) -> Result<Option<RunUsageReport>, RunUsageRepositoryError> {
        let transaction =
            self.database.begin().await.map_err(|source| {
                repository_error(database_error("begin run usage read", source))
            })?;
        let authority = match load_run_authority(&transaction, run_id, thread_id).await {
            Ok(authority) => authority,
            Err(error) => return rollback_with_error(transaction, error).await,
        };
        let result = load_current_usage(&transaction, run_id, thread_id, &authority).await;
        match result {
            Ok(usage) => {
                transaction.commit().await.map_err(|source| {
                    repository_error(database_error("commit run usage read", source))
                })?;
                Ok(usage.map(|stored| stored.report))
            }
            Err(error) => rollback_with_error(transaction, error).await,
        }
    }
}

fn validate_command_scope(command: &RecordRunUsage<'_>) -> Result<(), RunUsageRepositoryError> {
    if command.report.run_id() != command.run_id {
        return Err(RunUsageRepositoryError::ReportRunMismatch);
    }
    if command.report.thread_id() != command.thread_id {
        return Err(RunUsageRepositoryError::ReportThreadMismatch);
    }
    Ok(())
}

async fn load_run_authority(
    database: &impl ConnectionTrait,
    run_id: &RunId,
    thread_id: &ThreadId,
) -> Result<RunUsageAuthority, RunUsageRepositoryError> {
    let run = entities::assistant_run::Entity::find_by_id(run_id.as_str())
        .one(database)
        .await
        .map_err(|source| repository_error(database_error("load run usage authority", source)))?
        .ok_or_else(|| RunUsageRepositoryError::RunNotFound {
            run_id: run_id.clone(),
        })?;

    let persisted_thread_id = ThreadId::parse(run.thread_id).map_err(|_| {
        RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        }
    })?;
    if persisted_thread_id != *thread_id {
        return Err(RunUsageRepositoryError::RunThreadMismatch {
            run_id: run_id.clone(),
            thread_id: thread_id.clone(),
        });
    }
    if run.generation < 0
        || !matches!(run.engine_run_config_version, Some(1 | 2))
        || run
            .engine_run_config_revision
            .and_then(|value| u64::try_from(value).ok())
            .and_then(|value| EngineConfigRevision::new(value).ok())
            .is_none()
    {
        return Err(RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        });
    }
    let Some(blob) = run.engine_run_config.as_ref() else {
        return Err(RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        });
    };
    let config = engine_run_config::decode(blob.as_slice()).map_err(|_| {
        RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        }
    })?;
    let canonical = engine_run_config::encode(&config).map_err(|_| {
        RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        }
    })?;
    if canonical.as_slice() != blob.as_slice() {
        return Err(RunUsageRepositoryError::InvalidRunSnapshot {
            run_id: run_id.clone(),
        });
    }
    // Usage authority mirrors the immutable selection. OpenCode2 authorizes
    // its exact model/route/variant triple; Codex authorizes its exact model
    // on the `codex` route with no variant, matching `codex_usage_report`.
    // Any other engine, or a Codex selection with no model to fence against,
    // fails closed as an invalid snapshot instead of coercing to the wrong
    // engine. Scope mismatches against an established authority stay
    // `ModelOriginMismatch` in `report_matches_authority`.
    let (model_id, provider_route_id, variant_id) = match config.selection() {
        EngineSelection::OpenCode2(selection) => (
            selection.model_id().clone(),
            selection.route_id().clone(),
            selection.variant_id().cloned(),
        ),
        EngineSelection::Codex(selection) => {
            let Some(model_id) = selection.model_id().cloned() else {
                return Err(RunUsageRepositoryError::InvalidRunSnapshot {
                    run_id: run_id.clone(),
                });
            };
            let route_id = EngineRouteId::parse("codex").map_err(|_| {
                RunUsageRepositoryError::InvalidRunSnapshot {
                    run_id: run_id.clone(),
                }
            })?;
            (model_id, route_id, None)
        }
        _ => {
            return Err(RunUsageRepositoryError::InvalidRunSnapshot {
                run_id: run_id.clone(),
            });
        }
    };
    Ok(RunUsageAuthority {
        generation: run.generation,
        model_id,
        provider_route_id,
        variant_id,
    })
}

fn report_matches_authority(authority: &RunUsageAuthority, report: &RunUsageReport) -> bool {
    report.model_id() == &authority.model_id
        && report.provider_route_id() == &authority.provider_route_id
        && report.variant_id() == authority.variant_id.as_ref()
}

async fn record_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    authority: &RunUsageAuthority,
    command: &RecordRunUsage<'_>,
) -> Result<RecordRunUsageOutcome, RunUsageRepositoryError> {
    loop {
        let current =
            load_current_usage(transaction, command.run_id, command.thread_id, authority).await?;
        let Some(current) = current else {
            let inserted = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    INSERT_USAGE_SQL,
                    insert_values(command.report, authority.generation, command.run_id)?,
                ))
                .await
                .map_err(|source| repository_error(database_error("insert run usage", source)))?;
            if inserted.rows_affected() == 1 {
                return Ok(RecordRunUsageOutcome::Recorded(receipt(
                    command.run_id,
                    command.thread_id,
                    authority.generation,
                    command.report.source_sequence(),
                )));
            }
            // Another writer won the run's primary-key race. Re-read and
            // classify against its complete row instead of treating a no-op
            // insert as success.
            continue;
        };

        if current.report.provider_session_id() != command.report.provider_session_id() {
            return Err(RunUsageRepositoryError::ProviderSessionConflict {
                run_id: command.run_id.clone(),
            });
        }
        let incoming_sequence = command.report.source_sequence();
        let current_sequence = current.report.source_sequence();
        if incoming_sequence < current_sequence {
            return Err(RunUsageRepositoryError::StaleSequence {
                run_id: command.run_id.clone(),
                current: current_sequence,
                incoming: incoming_sequence,
            });
        }
        if incoming_sequence == current_sequence {
            if current.report.same_provider_measurement(command.report) {
                return Ok(RecordRunUsageOutcome::Duplicate(receipt(
                    command.run_id,
                    command.thread_id,
                    current.generation,
                    current_sequence,
                )));
            }
            return Err(RunUsageRepositoryError::SequenceConflict {
                run_id: command.run_id.clone(),
                sequence: current_sequence,
            });
        }

        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                UPDATE_USAGE_SQL,
                update_values(
                    command.report,
                    command.run_id,
                    command.thread_id,
                    authority.generation,
                    current.report.provider_session_id(),
                    current_sequence,
                )?,
            ))
            .await
            .map_err(|source| repository_error(database_error("replace run usage", source)))?;
        if updated.rows_affected() == 1 {
            return Ok(RecordRunUsageOutcome::Replaced(receipt(
                command.run_id,
                command.thread_id,
                authority.generation,
                incoming_sequence,
            )));
        }
        // The conditional update lost a concurrent race. Re-read and apply
        // the same provider-session/sequence classification.
    }
}

async fn load_current_usage(
    database: &impl ConnectionTrait,
    run_id: &RunId,
    thread_id: &ThreadId,
    authority: &RunUsageAuthority,
) -> Result<Option<StoredUsage>, RunUsageRepositoryError> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            SELECT_USAGE_SQL,
            [Value::String(Some(run_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| repository_error(database_error("load current run usage", source)))?;
    let Some(row) = row else {
        return Ok(None);
    };

    let stored_thread_id = ThreadId::parse(raw_string(&row, 0, run_id)?).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })?;
    if stored_thread_id != *thread_id {
        return Err(RunUsageRepositoryError::StoredThreadMismatch {
            run_id: run_id.clone(),
        });
    }
    let generation = raw_i64(&row, 1, run_id)?;
    if generation != authority.generation {
        return Err(RunUsageRepositoryError::GenerationMismatch {
            run_id: run_id.clone(),
        });
    }
    let report = decode_usage_row(&row, run_id, thread_id)?;
    if !report_matches_authority(authority, &report) {
        return Err(RunUsageRepositoryError::ModelOriginMismatch {
            run_id: run_id.clone(),
        });
    }
    Ok(Some(StoredUsage { generation, report }))
}

fn decode_usage_row(
    row: &sea_orm::QueryResult,
    run_id: &RunId,
    thread_id: &ThreadId,
) -> Result<RunUsageReport, RunUsageRepositoryError> {
    let provider_session_id = raw_string(row, 2, run_id)?;
    let source_sequence = u64::try_from(raw_i64(row, 3, run_id)?).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })?;
    let basis = RunUsageBasis::from_str(&raw_string(row, 4, run_id)?).ok_or_else(|| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })?;
    let provider_turn_id = raw_optional_string(row, 5, run_id)?;
    let model_id = EngineModelId::parse(raw_string(row, 6, run_id)?).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })?;
    let provider_route_id = EngineRouteId::parse(raw_string(row, 7, run_id)?).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })?;
    let variant_id = raw_optional_string(row, 8, run_id)?
        .map(EngineVariantId::parse)
        .transpose()
        .map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        })?;
    let input_tokens = raw_optional_i64(row, 9, run_id)?;
    let cached_input_tokens = raw_optional_i64(row, 10, run_id)?;
    let output_tokens = raw_optional_i64(row, 11, run_id)?;
    let context_tokens = raw_optional_i64(row, 12, run_id)?;
    let context_window_tokens = raw_optional_i64(row, 13, run_id)?;
    let observed_at = UnixMillis::from_millis(raw_i64(row, 14, run_id)?);

    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id,
        source_sequence,
        model_id,
        provider_route_id,
        variant_id,
        basis,
        provider_turn_id,
        input_tokens: bounded_token(input_tokens, run_id)?,
        cached_input_tokens: bounded_token(cached_input_tokens, run_id)?,
        output_tokens: bounded_token(output_tokens, run_id)?,
        context_tokens: bounded_token(context_tokens, run_id)?,
        context_window_tokens: bounded_token(context_window_tokens, run_id)?,
        observed_at,
    })
    .map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
        run_id: run_id.clone(),
    })
}

fn insert_values(
    report: &RunUsageReport,
    generation: i64,
    run_id: &RunId,
) -> Result<Vec<Value>, RunUsageRepositoryError> {
    Ok(vec![
        string_value(report.run_id().as_str()),
        string_value(report.thread_id().as_str()),
        Value::BigInt(Some(generation)),
        string_value(report.provider_session_id()),
        Value::BigInt(Some(storage_i64(report.source_sequence(), run_id)?)),
        string_value(report.basis().as_str()),
        optional_string_value(report.provider_turn_id()),
        string_value(report.model_id().as_str()),
        string_value(report.provider_route_id().as_str()),
        optional_string_value(report.variant_id().map(EngineVariantId::as_str)),
        optional_token_value(report.input_tokens(), run_id)?,
        optional_token_value(report.cached_input_tokens(), run_id)?,
        optional_token_value(report.output_tokens(), run_id)?,
        optional_token_value(report.context_tokens(), run_id)?,
        optional_token_value(report.context_window_tokens(), run_id)?,
        Value::BigInt(Some(report.observed_at().as_millis())),
    ])
}

fn update_values(
    report: &RunUsageReport,
    run_id: &RunId,
    thread_id: &ThreadId,
    generation: i64,
    expected_provider_session_id: &str,
    expected_sequence: u64,
) -> Result<Vec<Value>, RunUsageRepositoryError> {
    Ok(vec![
        string_value(report.provider_session_id()),
        Value::BigInt(Some(storage_i64(report.source_sequence(), run_id)?)),
        string_value(report.basis().as_str()),
        optional_string_value(report.provider_turn_id()),
        string_value(report.model_id().as_str()),
        string_value(report.provider_route_id().as_str()),
        optional_string_value(report.variant_id().map(EngineVariantId::as_str)),
        optional_token_value(report.input_tokens(), run_id)?,
        optional_token_value(report.cached_input_tokens(), run_id)?,
        optional_token_value(report.output_tokens(), run_id)?,
        optional_token_value(report.context_tokens(), run_id)?,
        optional_token_value(report.context_window_tokens(), run_id)?,
        Value::BigInt(Some(report.observed_at().as_millis())),
        string_value(run_id.as_str()),
        string_value(thread_id.as_str()),
        Value::BigInt(Some(generation)),
        string_value(expected_provider_session_id),
        Value::BigInt(Some(storage_i64(expected_sequence, run_id)?)),
        Value::BigInt(Some(storage_i64(report.source_sequence(), run_id)?)),
    ])
}

fn storage_i64(value: u64, run_id: &RunId) -> Result<i64, RunUsageRepositoryError> {
    i64::try_from(value).map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
        run_id: run_id.clone(),
    })
}

fn optional_token_value(
    value: Option<u64>,
    run_id: &RunId,
) -> Result<Value, RunUsageRepositoryError> {
    Ok(Value::BigInt(
        value.map(|value| storage_i64(value, run_id)).transpose()?,
    ))
}

fn bounded_token(
    value: Option<i64>,
    run_id: &RunId,
) -> Result<Option<u64>, RunUsageRepositoryError> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
                run_id: run_id.clone(),
            })
        })
        .transpose()
}

fn receipt(
    run_id: &RunId,
    thread_id: &ThreadId,
    generation: i64,
    source_sequence: u64,
) -> RunUsageWriteReceipt {
    RunUsageWriteReceipt {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        generation,
        source_sequence,
    }
}

fn string_value(value: &str) -> Value {
    Value::String(Some(value.to_owned()))
}

fn optional_string_value(value: Option<&str>) -> Value {
    Value::String(value.map(str::to_owned))
}

fn raw_string(
    row: &sea_orm::QueryResult,
    index: usize,
    run_id: &RunId,
) -> Result<String, RunUsageRepositoryError> {
    row.try_get_by_index::<String>(index)
        .map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        })
}

fn raw_optional_string(
    row: &sea_orm::QueryResult,
    index: usize,
    run_id: &RunId,
) -> Result<Option<String>, RunUsageRepositoryError> {
    row.try_get_by_index::<Option<String>>(index).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })
}

fn raw_i64(
    row: &sea_orm::QueryResult,
    index: usize,
    run_id: &RunId,
) -> Result<i64, RunUsageRepositoryError> {
    row.try_get_by_index::<i64>(index)
        .map_err(|_| RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        })
}

fn raw_optional_i64(
    row: &sea_orm::QueryResult,
    index: usize,
    run_id: &RunId,
) -> Result<Option<i64>, RunUsageRepositoryError> {
    row.try_get_by_index::<Option<i64>>(index).map_err(|_| {
        RunUsageRepositoryError::CorruptUsageRow {
            run_id: run_id.clone(),
        }
    })
}

fn repository_error(error: RepositoryError) -> RunUsageRepositoryError {
    RunUsageRepositoryError::Repository(error)
}

async fn rollback_with_error<T>(
    transaction: sea_orm::DatabaseTransaction,
    error: RunUsageRepositoryError,
) -> Result<T, RunUsageRepositoryError> {
    transaction.rollback().await.map_err(|source| {
        repository_error(database_error("roll back run usage transaction", source))
    })?;
    Err(error)
}
