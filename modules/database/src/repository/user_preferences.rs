//! The Forge user's preferences and navigation record.
//!
//! One singleton row holds the default engine configuration, the last
//! route, and a revision that grows with every change; `navigation_projects`
//! holds one row per used project with its recency (the revision at its last
//! use) and the thread last open in it. Every write runs in one `IMMEDIATE`
//! transaction and bumps the revision only when something changed, so a
//! repeated report is free.

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, Value};

use artisan_domain::{
    EngineRunConfig, LegacyImportOutcome, NavigationProject, NavigationRecord, NavigationRoute,
    ProjectId, ThreadId, UnixMillis,
};

use super::{Repository, RepositoryError, corrupt_data, database_error};

const PREFERENCES_SELECT: &str = "SELECT revision, default_engine_config, route_project_id, route_thread_id FROM user_preferences WHERE state_id = 1";
const PROJECTS_SELECT: &str = "SELECT project_id, last_thread_id FROM navigation_projects ORDER BY recency DESC, project_id ASC LIMIT ?";

/// The stored preferences: everything but the live account profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredUserPreferences {
    /// Revision; grows with every change.
    pub revision: u64,
    /// Configuration new threads start from.
    pub default_engine_config: Option<EngineRunConfig>,
    /// Project order, last threads, and route.
    pub navigation: NavigationRecord,
}

/// What a legacy import adopted, and the preferences after it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyImport {
    /// Outcome for the default engine configuration.
    pub default_model: LegacyImportOutcome,
    /// Outcome for the project order.
    pub project_order: LegacyImportOutcome,
    /// Preferences after the import.
    pub preferences: StoredUserPreferences,
}

type PreferencesResult<T> = Result<T, RepositoryError>;

impl Repository {
    /// Reads the stored preferences.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn read_user_preferences(&self) -> PreferencesResult<StoredUserPreferences> {
        read_preferences(&self.database).await
    }

    /// Records that the user opened `project` (and `thread` in it): the
    /// project becomes the most recently used, `thread` becomes its
    /// remembered thread, and the route points at the project and its
    /// remembered thread.
    ///
    /// # Errors
    ///
    /// Returns an unknown-project or unknown-thread failure (a thread of
    /// another project is unknown here), or a database failure.
    pub async fn record_navigation(
        &self,
        project: &ProjectId,
        thread: Option<&ThreadId>,
        at: UnixMillis,
    ) -> PreferencesResult<StoredUserPreferences> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin navigation record", source))?;
        let result = apply_navigation(&transaction, project, thread, at).await;
        finish(transaction, result, "navigation record").await
    }

    /// Makes `config` the default engine configuration. It follows a thread
    /// save that already took its own acceptance instant, so it records no
    /// time of its own.
    ///
    /// # Errors
    ///
    /// Returns an encoding or database failure.
    pub async fn remember_default_engine_config(
        &self,
        config: &EngineRunConfig,
    ) -> PreferencesResult<StoredUserPreferences> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin default engine config", source))?;
        let result = apply_default(&transaction, config).await;
        finish(transaction, result, "default engine config").await
    }

    /// Adopts preferences an older Editor kept in files, each only where the
    /// Forge has none: the default configuration when none is set, and the
    /// project order (unknown projects dropped) when no project was used yet.
    /// `default` is `None` when no model was sent and `Some(None)` when the
    /// sent model could not be resolved.
    ///
    /// # Errors
    ///
    /// Returns an encoding or database failure.
    pub async fn import_legacy_preferences(
        &self,
        default: Option<Option<&EngineRunConfig>>,
        order: &[ProjectId],
        at: UnixMillis,
    ) -> PreferencesResult<LegacyImport> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin legacy preference import", source))?;
        let result = apply_import(&transaction, default, order, at).await;
        finish(transaction, result, "legacy preference import").await
    }
}

async fn finish<T>(
    transaction: DatabaseTransaction,
    result: PreferencesResult<T>,
    operation: &'static str,
) -> PreferencesResult<T> {
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

async fn apply_navigation(
    transaction: &DatabaseTransaction,
    project: &ProjectId,
    thread: Option<&ThreadId>,
    at: UnixMillis,
) -> PreferencesResult<StoredUserPreferences> {
    let exists = transaction
        .query_one_raw(statement(
            "SELECT 1 FROM attached_projects WHERE project_id = ?",
            [text(project.as_str())],
        ))
        .await
        .map_err(|source| database_error("read navigation project", source))?;
    if exists.is_none() {
        return Err(RepositoryError::ProjectNotFound {
            project_id: project.clone(),
        });
    }
    if let Some(thread) = thread {
        let owned = transaction
            .query_one_raw(statement(
                "SELECT 1 FROM threads WHERE thread_id = ? AND project_id = ?",
                [text(thread.as_str()), text(project.as_str())],
            ))
            .await
            .map_err(|source| database_error("read navigation thread", source))?;
        if owned.is_none() {
            return Err(RepositoryError::ThreadNotFound {
                thread_id: thread.clone(),
            });
        }
    }
    let current = read_preferences(transaction).await?;
    let remembered = thread
        .cloned()
        .or_else(|| current.navigation.last_thread(project).cloned());
    let unchanged =
        current.navigation.projects().first().is_some_and(|first| {
            &first.project_id == project && first.last_thread_id == remembered
        }) && current.navigation.route()
            == Some(&NavigationRoute {
                project_id: project.clone(),
                thread_id: remembered.clone(),
            });
    if unchanged {
        return Ok(current);
    }
    let revision = next_revision(current.revision)?;
    execute(
        transaction,
        "INSERT INTO navigation_projects (project_id, recency, last_thread_id) VALUES (?, ?, ?) \
         ON CONFLICT(project_id) DO UPDATE SET recency = excluded.recency, last_thread_id = excluded.last_thread_id",
        [
            text(project.as_str()),
            integer(revision)?,
            optional_text(remembered.as_ref().map(ThreadId::as_str)),
        ],
        "record navigation project",
    )
    .await?;
    execute(
        transaction,
        "UPDATE user_preferences SET revision = ?, route_project_id = ?, route_thread_id = ?, updated_at_ms = ? WHERE state_id = 1",
        [
            integer(revision)?,
            text(project.as_str()),
            optional_text(remembered.as_ref().map(ThreadId::as_str)),
            Value::BigInt(Some(at.as_millis())),
        ],
        "record navigation route",
    )
    .await?;
    read_preferences(transaction).await
}

async fn apply_default(
    transaction: &DatabaseTransaction,
    config: &EngineRunConfig,
) -> PreferencesResult<StoredUserPreferences> {
    let current = read_preferences(transaction).await?;
    if current.default_engine_config.as_ref() == Some(config) {
        return Ok(current);
    }
    write_default(transaction, config, next_revision(current.revision)?).await?;
    read_preferences(transaction).await
}

async fn write_default(
    transaction: &DatabaseTransaction,
    config: &EngineRunConfig,
    revision: u64,
) -> PreferencesResult<()> {
    let encoded = crate::engine_run_config::encode(config)
        .map_err(|error| corrupt_data("user_preferences", "default_engine_config", error))?;
    execute(
        transaction,
        "UPDATE user_preferences SET revision = ?, default_engine_config = ? WHERE state_id = 1",
        [integer(revision)?, Value::Bytes(Some(encoded))],
        "store default engine config",
    )
    .await
}

async fn apply_import(
    transaction: &DatabaseTransaction,
    default: Option<Option<&EngineRunConfig>>,
    order: &[ProjectId],
    at: UnixMillis,
) -> PreferencesResult<LegacyImport> {
    let current = read_preferences(transaction).await?;
    let mut revision = current.revision;
    let default_model = match default {
        None => LegacyImportOutcome::Absent,
        Some(_) if current.default_engine_config.is_some() => LegacyImportOutcome::Kept,
        Some(None) => LegacyImportOutcome::Refused,
        Some(Some(config)) => {
            revision = next_revision(revision)?;
            write_default(transaction, config, revision).await?;
            LegacyImportOutcome::Imported
        }
    };
    let project_order = if order.is_empty() {
        LegacyImportOutcome::Absent
    } else if !current.navigation.projects().is_empty() {
        LegacyImportOutcome::Kept
    } else {
        let mut imported = false;
        // Oldest first, so the first project in the file ends most recent.
        for project in order.iter().rev() {
            revision = next_revision(revision)?;
            let inserted = transaction
                .execute_raw(statement(
                    "INSERT OR IGNORE INTO navigation_projects (project_id, recency, last_thread_id) \
                     SELECT project_id, ?, NULL FROM attached_projects WHERE project_id = ?",
                    [integer(revision)?, text(project.as_str())],
                ))
                .await
                .map_err(|source| database_error("import navigation project", source))?;
            imported |= inserted.rows_affected() == 1;
        }
        if imported {
            execute(
                transaction,
                "UPDATE user_preferences SET revision = ?, updated_at_ms = ? WHERE state_id = 1",
                [integer(revision)?, Value::BigInt(Some(at.as_millis()))],
                "advance imported preferences",
            )
            .await?;
            LegacyImportOutcome::Imported
        } else {
            LegacyImportOutcome::Refused
        }
    };
    Ok(LegacyImport {
        default_model,
        project_order,
        preferences: read_preferences(transaction).await?,
    })
}

async fn read_preferences(
    database: &impl ConnectionTrait,
) -> PreferencesResult<StoredUserPreferences> {
    let row = database
        .query_one_raw(statement(PREFERENCES_SELECT, []))
        .await
        .map_err(|source| database_error("read user preferences", source))?
        .ok_or(RepositoryError::Invariant {
            reason: "user preferences row is missing",
        })?;
    let revision = row
        .try_get_by_index::<i64>(0)
        .map_err(|error| corrupt_data("user_preferences", "revision", error))
        .and_then(|value| {
            u64::try_from(value)
                .map_err(|error| corrupt_data("user_preferences", "revision", error))
        })?;
    let default_engine_config = row
        .try_get_by_index::<Option<Vec<u8>>>(1)
        .map_err(|error| corrupt_data("user_preferences", "default_engine_config", error))?
        .map(|bytes| {
            crate::engine_run_config::decode(&bytes)
                .map_err(|error| corrupt_data("user_preferences", "default_engine_config", error))
        })
        .transpose()?;
    let route_project = optional_id(&row, 2, "route_project_id", ProjectId::parse)?;
    let route_thread = optional_id(&row, 3, "route_thread_id", ThreadId::parse)?;
    let limit = i64::try_from(artisan_domain::NAVIGATION_PROJECTS_MAX).unwrap_or(i64::MAX);
    let projects = database
        .query_all_raw(statement(PROJECTS_SELECT, [Value::BigInt(Some(limit))]))
        .await
        .map_err(|source| database_error("read navigation projects", source))?
        .iter()
        .map(|row| {
            Ok(NavigationProject {
                project_id: optional_id(row, 0, "project_id", ProjectId::parse)?.ok_or_else(
                    || corrupt_data("navigation_projects", "project_id", "missing project"),
                )?,
                last_thread_id: optional_id(row, 1, "last_thread_id", ThreadId::parse)?,
            })
        })
        .collect::<PreferencesResult<Vec<_>>>()?;
    let route = route_project.map(|project_id| NavigationRoute {
        project_id,
        thread_id: route_thread,
    });
    let navigation = NavigationRecord::new(projects, route)
        .map_err(|error| corrupt_data("navigation_projects", "project_id", error))?;
    Ok(StoredUserPreferences {
        revision,
        default_engine_config,
        navigation,
    })
}

fn optional_id<T, E: ToString>(
    row: &sea_orm::QueryResult,
    index: usize,
    field: &'static str,
    parse: impl FnOnce(String) -> Result<T, E>,
) -> PreferencesResult<Option<T>> {
    row.try_get_by_index::<Option<String>>(index)
        .map_err(|error| corrupt_data("user_preferences", field, error))?
        .map(|value| parse(value).map_err(|error| corrupt_data("user_preferences", field, error)))
        .transpose()
}

fn next_revision(current: u64) -> PreferencesResult<u64> {
    current
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
        .ok_or(RepositoryError::Invariant {
            reason: "user preferences revision cannot advance",
        })
}

fn integer(value: u64) -> PreferencesResult<Value> {
    i64::try_from(value)
        .map(|value| Value::BigInt(Some(value)))
        .map_err(|_| RepositoryError::Invariant {
            reason: "user preferences revision exceeds its storage",
        })
}

fn text(value: &str) -> Value {
    Value::String(Some(value.to_owned()))
}

fn optional_text(value: Option<&str>) -> Value {
    Value::String(value.map(str::to_owned))
}

fn statement<const N: usize>(sql: &str, values: [Value; N]) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

async fn execute<const N: usize>(
    transaction: &DatabaseTransaction,
    sql: &str,
    values: [Value; N],
    operation: &'static str,
) -> PreferencesResult<()> {
    transaction
        .execute_raw(statement(sql, values))
        .await
        .map(|_| ())
        .map_err(|source| database_error(operation, source))
}
