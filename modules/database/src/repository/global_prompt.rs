//! The user's global prompt, its saved versions, and what Artisan last wrote
//! to each engine home.
//!
//! The singleton row holds the canonical text and a revision that grows with
//! every saved version. A save names the revision it edited, so a save over a
//! newer revision is reported stale rather than silently replacing someone
//! else's edit. Versions are kept up to [`GLOBAL_PROMPT_VERSIONS_KEPT`].

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, Value};

use artisan_domain::UnixMillis;

use super::{Repository, RepositoryError, corrupt_data, database_error};

/// Saved versions kept; older ones are pruned on save.
pub const GLOBAL_PROMPT_VERSIONS_KEPT: u64 = 100;

/// The canonical prompt as stored.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoredGlobalPrompt {
    /// Revision; grows with every saved version.
    pub revision: u64,
    /// Whether setup imported or chose the prompt.
    pub established: bool,
    /// The canonical text.
    pub text: String,
}

/// The outcome of a save.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GlobalPromptSave {
    /// A new version was saved.
    Saved(StoredGlobalPrompt),
    /// The text was already canonical; nothing changed.
    Unchanged(StoredGlobalPrompt),
    /// The save edited an older revision; the current prompt is returned.
    Stale(StoredGlobalPrompt),
}

/// What Artisan last wrote to one engine home's global file.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoredPromptFile {
    /// Engine identifier.
    pub engine_id: String,
    /// The home, relative to the engine's toolchain folder.
    pub home_key: String,
    /// Digest of the normalized text written.
    pub written_digest: String,
}

impl Repository {
    /// Reads the canonical prompt.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn read_global_prompt(&self) -> Result<StoredGlobalPrompt, RepositoryError> {
        read_prompt(&self.database).await
    }

    /// Establishes the canonical prompt once setup settled on `text`. An
    /// established prompt is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn establish_global_prompt(
        &self,
        text: &str,
        at: UnixMillis,
    ) -> Result<StoredGlobalPrompt, RepositoryError> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin global prompt setup", source))?;
        let result = async {
            let current = read_prompt(&transaction).await?;
            if current.established {
                return Ok(current);
            }
            if text == current.text {
                execute(
                    &transaction,
                    "UPDATE global_prompt SET established = 1, updated_at_ms = ? WHERE state_id = 1",
                    [millis(at)],
                    "establish global prompt",
                )
                .await?;
                return read_prompt(&transaction).await;
            }
            write_version(&transaction, &current, text, at).await?;
            execute(
                &transaction,
                "UPDATE global_prompt SET established = 1 WHERE state_id = 1",
                [],
                "establish global prompt",
            )
            .await?;
            read_prompt(&transaction).await
        }
        .await;
        finish(transaction, result, "global prompt setup").await
    }

    /// Saves `text` as a new version when it differs from the canonical
    /// prompt and `base_revision` is current.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn save_global_prompt(
        &self,
        base_revision: u64,
        text: &str,
        at: UnixMillis,
    ) -> Result<GlobalPromptSave, RepositoryError> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin global prompt save", source))?;
        let result = async {
            let current = read_prompt(&transaction).await?;
            if current.text == text {
                return Ok(GlobalPromptSave::Unchanged(current));
            }
            if current.revision != base_revision {
                return Ok(GlobalPromptSave::Stale(current));
            }
            write_version(&transaction, &current, text, at).await?;
            execute(
                &transaction,
                "UPDATE global_prompt SET established = 1 WHERE state_id = 1",
                [],
                "establish global prompt",
            )
            .await?;
            Ok(GlobalPromptSave::Saved(read_prompt(&transaction).await?))
        }
        .await;
        finish(transaction, result, "global prompt save").await
    }

    /// Reads what Artisan last wrote to each engine home.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn read_prompt_files(&self) -> Result<Vec<StoredPromptFile>, RepositoryError> {
        let rows = self
            .database
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT engine_id, home_key, written_digest FROM global_prompt_engine_files ORDER BY engine_id, home_key",
            ))
            .await
            .map_err(|source| database_error("read global prompt files", source))?;
        rows.iter()
            .map(|row| {
                let column = |index: usize, field: &'static str| {
                    row.try_get_by_index::<String>(index)
                        .map_err(|error| corrupt_data("global_prompt_engine_files", field, error))
                };
                Ok(StoredPromptFile {
                    engine_id: column(0, "engine_id")?,
                    home_key: column(1, "home_key")?,
                    written_digest: column(2, "written_digest")?,
                })
            })
            .collect()
    }

    /// Records that Artisan wrote the text with `digest` to one engine home.
    ///
    /// # Errors
    ///
    /// Returns a database failure.
    pub async fn record_prompt_file(
        &self,
        file: &StoredPromptFile,
        at: UnixMillis,
    ) -> Result<(), RepositoryError> {
        let _write_gate = self.acquire_write_gate().await;
        self.database
            .execute_raw(statement(
                "INSERT INTO global_prompt_engine_files (engine_id, home_key, written_digest, written_at_ms) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(engine_id, home_key) DO UPDATE SET written_digest = excluded.written_digest, written_at_ms = excluded.written_at_ms",
                [
                    text(&file.engine_id),
                    text(&file.home_key),
                    text(&file.written_digest),
                    millis(at),
                ],
            ))
            .await
            .map(|_| ())
            .map_err(|source| database_error("record global prompt file", source))
    }
}

async fn read_prompt(
    connection: &impl ConnectionTrait,
) -> Result<StoredGlobalPrompt, RepositoryError> {
    let row = connection
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT revision, established, text FROM global_prompt WHERE state_id = 1",
        ))
        .await
        .map_err(|source| database_error("read global prompt", source))?
        .ok_or(RepositoryError::Invariant {
            reason: "the global prompt row is missing",
        })?;
    let revision = row
        .try_get_by_index::<i64>(0)
        .map_err(|error| corrupt_data("global_prompt", "revision", error))
        .and_then(|value| {
            u64::try_from(value).map_err(|error| corrupt_data("global_prompt", "revision", error))
        })?;
    let established = row
        .try_get_by_index::<i64>(1)
        .map_err(|error| corrupt_data("global_prompt", "established", error))?;
    let text = row
        .try_get_by_index::<String>(2)
        .map_err(|error| corrupt_data("global_prompt", "text", error))?;
    Ok(StoredGlobalPrompt {
        revision,
        established: established != 0,
        text,
    })
}

/// Saves `text` as the next version and makes it canonical.
async fn write_version(
    transaction: &DatabaseTransaction,
    current: &StoredGlobalPrompt,
    body: &str,
    at: UnixMillis,
) -> Result<(), RepositoryError> {
    let next = current
        .revision
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
        .ok_or(RepositoryError::Invariant {
            reason: "global prompt revision cannot advance",
        })?;
    let next_value = Value::BigInt(Some(i64::try_from(next).unwrap_or(i64::MAX)));
    execute(
        transaction,
        "UPDATE global_prompt SET revision = ?, text = ?, updated_at_ms = ? WHERE state_id = 1",
        [next_value.clone(), text(body), millis(at)],
        "save global prompt",
    )
    .await?;
    execute(
        transaction,
        "INSERT INTO global_prompt_versions (revision, text, saved_at_ms) VALUES (?, ?, ?)",
        [next_value, text(body), millis(at)],
        "save global prompt version",
    )
    .await?;
    let oldest_kept = next.saturating_sub(GLOBAL_PROMPT_VERSIONS_KEPT);
    execute(
        transaction,
        "DELETE FROM global_prompt_versions WHERE revision <= ?",
        [Value::BigInt(Some(i64::try_from(oldest_kept).unwrap_or(0)))],
        "prune global prompt versions",
    )
    .await
}

async fn finish<T>(
    transaction: crate::repository::WriteTransaction,
    result: Result<T, RepositoryError>,
    operation: &'static str,
) -> Result<T, RepositoryError> {
    match result {
        Ok(value) => {
            transaction
                .commit()
                .await
                .map_err(|source| database_error(operation, source))?;
            Ok(value)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

async fn execute<const N: usize>(
    transaction: &DatabaseTransaction,
    sql: &str,
    values: [Value; N],
    operation: &'static str,
) -> Result<(), RepositoryError> {
    transaction
        .execute_raw(statement(sql, values))
        .await
        .map(|_| ())
        .map_err(|source| database_error(operation, source))
}

fn statement<const N: usize>(sql: &str, values: [Value; N]) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

fn text(value: &str) -> Value {
    Value::String(Some(value.to_owned()))
}

fn millis(at: UnixMillis) -> Value {
    Value::BigInt(Some(at.as_millis()))
}
