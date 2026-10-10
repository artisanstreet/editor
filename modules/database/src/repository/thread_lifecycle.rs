//! Where each thread lives: in view, settled into History, or archived, and
//! the Forge's upkeep of both: filing quiet threads into History and erasing
//! threads archived for longer than the retention period.

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait, QueryFilter,
    Statement, Value,
};

use artisan_domain::{
    ARCHIVE_RETENTION_MS, HISTORY_IDLE_MS, ThreadChange, ThreadId, ThreadSummary, UnixMillis,
};

use crate::entities;

use super::project_threads::thread_summary;
use super::{Repository, RepositoryError, database_error};

/// A thread's latest activity as the recent listing orders it: its latest
/// message, else its last update.
pub(super) const THREAD_ACTIVITY_SQL: &str = "COALESCE((SELECT MAX(m.accepted_at_ms) FROM messages AS m \
     WHERE m.thread_id = t.thread_id), t.updated_at_ms)";

/// Whether a run the Forge still owns belongs to the thread.
const LIVE_RUN_SQL: &str = "EXISTS (SELECT 1 FROM assistant_runs AS r WHERE r.thread_id = t.thread_id \
     AND r.lifecycle IN ('queued', 'launching', 'running', 'waiting', 'cancel_requested'))";

/// Files every thread untouched since the cutoff into History: no message,
/// no settled run, and no viewing after it. Live threads, archived threads,
/// the thread the reader has open, and threads already filed since their
/// latest activity are left alone.
fn file_quiet_threads_sql() -> String {
    format!(
        "INSERT INTO thread_lifecycle (thread_id, history_at_ms) \
         SELECT t.thread_id, ? FROM threads AS t \
         LEFT JOIN thread_lifecycle AS l ON l.thread_id = t.thread_id \
         WHERE l.archived_at_ms IS NULL \
           AND NOT {LIVE_RUN_SQL} \
           AND t.thread_id IS NOT (SELECT route_thread_id FROM user_preferences LIMIT 1) \
           AND (l.history_at_ms IS NULL OR l.history_at_ms < {THREAD_ACTIVITY_SQL}) \
           AND MAX({THREAD_ACTIVITY_SQL}, \
                   COALESCE((SELECT read_at_ms FROM thread_reads AS tr WHERE tr.thread_id = t.thread_id), 0), \
                   COALESCE((SELECT MAX(COALESCE(r.terminal_at_ms, r.updated_at_ms)) FROM assistant_runs AS r \
                             WHERE r.thread_id = t.thread_id), 0)) <= ? \
         ON CONFLICT(thread_id) DO UPDATE SET history_at_ms = excluded.history_at_ms"
    )
}

/// Tables holding a thread's rows, children first, each with the condition
/// selecting the purged thread's rows (`?` is its id). Rows of other
/// threads that point at the purged thread's messages go with them.
const ERASE_STATEMENTS: &[&str] = &[
    "DELETE FROM conversation_patches WHERE thread_id = ?",
    "DELETE FROM observation_ledger WHERE thread_id = ?",
    "DELETE FROM run_usage WHERE thread_id = ?",
    "DELETE FROM run_checkpoints WHERE run_id IN (SELECT run_id FROM assistant_runs WHERE thread_id = ?)",
    "DELETE FROM run_batch_receipts WHERE run_id IN (SELECT run_id FROM assistant_runs WHERE thread_id = ?)",
    "DELETE FROM pending_run_interactions WHERE thread_id = ?",
    "DELETE FROM run_interaction_receipts WHERE thread_id = ?",
    "DELETE FROM conversation_items WHERE thread_id = ?",
    "DELETE FROM command_receipts WHERE thread_id = ?1 \
     OR message_id IN (SELECT message_id FROM messages WHERE thread_id = ?1)",
    "DELETE FROM failed_message_recoveries WHERE thread_id = ?1 OR new_thread_id = ?1 \
     OR message_id IN (SELECT message_id FROM messages WHERE thread_id = ?1)",
    "DELETE FROM composer_draft_submissions WHERE thread_id = ?1 \
     OR message_id IN (SELECT message_id FROM messages WHERE thread_id = ?1)",
    "DELETE FROM project_draft_submissions WHERE thread_id = ?1 \
     OR message_id IN (SELECT message_id FROM messages WHERE thread_id = ?1)",
    "DELETE FROM assistant_runs WHERE thread_id = ?",
    "DELETE FROM conversation_turns WHERE thread_id = ?",
    "DELETE FROM conversation_ordinals WHERE thread_id = ?",
    "DELETE FROM conversation_state WHERE thread_id = ?",
    "DELETE FROM queued_message_withdrawals WHERE thread_id = ?",
    "DELETE FROM message_dispatches WHERE message_id IN (SELECT message_id FROM messages WHERE thread_id = ?)",
    "DELETE FROM message_image_attachments WHERE message_id IN (SELECT message_id FROM messages WHERE thread_id = ?)",
    "DELETE FROM messages WHERE thread_id = ?",
    "DELETE FROM composer_drafts WHERE scope_kind = 'thread' AND scope_id = ?",
    "DELETE FROM composer_draft_save_receipts WHERE scope_kind = 'thread' AND scope_id = ?",
    // `thread_reads` and `thread_lifecycle` cascade; the navigation record
    // forgets the thread on its own (`SET NULL`).
    "DELETE FROM threads WHERE thread_id = ?",
];

impl Repository {
    /// Applies one change the reader made to a thread at `at`.
    ///
    /// A rename keeps the reader's title from then on. Settling files the
    /// thread into History until its next activity. Archiving takes it out
    /// of every listing; restoring it brings it back into view as though
    /// the reader had just looked at it, so it is not filed straight away.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::ThreadNotFound`] for an unknown thread, or
    /// a preserved database failure.
    pub async fn update_thread(
        &self,
        thread: &ThreadId,
        change: &ThreadChange,
        at: UnixMillis,
    ) -> Result<(), RepositoryError> {
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin thread update", source))?;
        let result = apply_change(&transaction, thread, change, at).await;
        match result {
            Ok(()) => transaction
                .commit()
                .await
                .map_err(|source| database_error("commit thread update", source)),
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// Lists the archived threads with when each was archived, oldest
    /// first, at most `limit` of them.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn list_archived_threads(
        &self,
        limit: usize,
    ) -> Result<Vec<(ThreadSummary, UnixMillis)>, RepositoryError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let archived = self
            .database
            .query_all_raw(statement(
                "SELECT thread_id, archived_at_ms FROM thread_lifecycle \
                 WHERE archived_at_ms IS NOT NULL ORDER BY archived_at_ms ASC, thread_id ASC LIMIT ?",
                [Value::BigInt(Some(limit))],
            ))
            .await
            .map_err(|source| database_error("list archived threads", source))?
            .iter()
            .map(|row| {
                let id = row
                    .try_get_by_index::<String>(0)
                    .map_err(|source| database_error("read archived thread id", source))?;
                let at = row
                    .try_get_by_index::<i64>(1)
                    .map_err(|source| database_error("read archived thread time", source))?;
                Ok((id, UnixMillis::from_millis(at)))
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        if archived.is_empty() {
            return Ok(Vec::new());
        }
        let rows = entities::thread::Entity::find()
            .filter(
                entities::thread::Column::ThreadId
                    .is_in(archived.iter().map(|(id, _)| id.as_str())),
            )
            .all(&self.database)
            .await
            .map_err(|source| database_error("read archived threads", source))?;
        let mut summaries = rows
            .into_iter()
            .map(thread_summary)
            .collect::<Result<Vec<_>, _>>()?;
        self.project_listed_threads(&mut summaries).await?;
        Ok(archived
            .into_iter()
            .filter_map(|(id, at)| {
                let index = summaries
                    .iter()
                    .position(|summary| summary.thread_id.as_str() == id)?;
                Some((summaries.swap_remove(index), at))
            })
            .collect())
    }

    /// Files every thread that has gone [`HISTORY_IDLE_MS`] without
    /// activity or viewing into History at `at`, returning how many moved.
    ///
    /// # Errors
    ///
    /// Returns a preserved database failure.
    pub async fn file_quiet_threads(&self, at: UnixMillis) -> Result<u64, RepositoryError> {
        let cutoff = at.as_millis().saturating_sub(HISTORY_IDLE_MS);
        let _write_gate = self.acquire_write_gate().await;
        self.database
            .execute_raw(statement(
                &file_quiet_threads_sql(),
                [
                    Value::BigInt(Some(at.as_millis())),
                    Value::BigInt(Some(cutoff)),
                ],
            ))
            .await
            .map(|result| result.rows_affected())
            .map_err(|source| database_error("file quiet threads", source))
    }

    /// Erases, for good, up to `limit` threads archived at least
    /// [`ARCHIVE_RETENTION_MS`] before `at`, oldest first, returning them.
    /// A thread with a run the Forge still owns waits for the run to end.
    /// Each thread is erased in its own transaction.
    ///
    /// # Errors
    ///
    /// Returns a preserved database failure; threads erased before it stay
    /// erased.
    pub async fn erase_expired_archived_threads(
        &self,
        at: UnixMillis,
        limit: usize,
    ) -> Result<Vec<ThreadId>, RepositoryError> {
        let cutoff = at.as_millis().saturating_sub(ARCHIVE_RETENTION_MS);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let ids = self
            .database
            .query_all_raw(statement(
                &format!(
                    "SELECT l.thread_id FROM thread_lifecycle AS l \
                     JOIN threads AS t ON t.thread_id = l.thread_id \
                     WHERE l.archived_at_ms IS NOT NULL AND l.archived_at_ms <= ? \
                       AND NOT {LIVE_RUN_SQL} \
                     ORDER BY l.archived_at_ms ASC, l.thread_id ASC LIMIT ?"
                ),
                [Value::BigInt(Some(cutoff)), Value::BigInt(Some(limit))],
            ))
            .await
            .map_err(|source| database_error("list expired archived threads", source))?
            .iter()
            .map(|row| {
                let id = row
                    .try_get_by_index::<String>(0)
                    .map_err(|source| database_error("read expired archived thread", source))?;
                ThreadId::parse(id).map_err(|_| RepositoryError::Invariant {
                    reason: "stored thread id is not a valid thread id",
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut erased = Vec::with_capacity(ids.len());
        for id in ids {
            let transaction = self
                .begin_write()
                .await
                .map_err(|source| database_error("begin archived thread erase", source))?;
            let result = erase_thread(&transaction, &id).await;
            match result {
                Ok(()) => {
                    transaction
                        .commit()
                        .await
                        .map_err(|source| database_error("commit archived thread erase", source))?;
                    erased.push(id);
                }
                Err(error) => {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            }
        }
        Ok(erased)
    }
}

async fn apply_change(
    transaction: &DatabaseTransaction,
    thread: &ThreadId,
    change: &ThreadChange,
    at: UnixMillis,
) -> Result<(), RepositoryError> {
    let exists = transaction
        .query_one_raw(statement(
            "SELECT 1 FROM threads WHERE thread_id = ?",
            [text(thread.as_str())],
        ))
        .await
        .map_err(|source| database_error("read updated thread", source))?;
    if exists.is_none() {
        return Err(RepositoryError::ThreadNotFound {
            thread_id: thread.clone(),
        });
    }
    let id = || text(thread.as_str());
    let now = || Value::BigInt(Some(at.as_millis()));
    match change {
        ThreadChange::Rename(title) => {
            execute(
                transaction,
                "UPDATE threads SET title = ? WHERE thread_id = ?",
                [text(title.as_str()), id()],
                "rename thread",
            )
            .await?;
            execute(
                transaction,
                "INSERT INTO thread_lifecycle (thread_id, titled_by_reader) VALUES (?, 1) \
                 ON CONFLICT(thread_id) DO UPDATE SET titled_by_reader = 1",
                [id()],
                "keep the reader's title",
            )
            .await
        }
        ThreadChange::Settle => {
            execute(
                transaction,
                "INSERT INTO thread_lifecycle (thread_id, history_at_ms) VALUES (?, ?) \
                 ON CONFLICT(thread_id) DO UPDATE SET history_at_ms = excluded.history_at_ms",
                [id(), now()],
                "settle thread",
            )
            .await
        }
        ThreadChange::Archive => {
            execute(
                transaction,
                "INSERT INTO thread_lifecycle (thread_id, archived_at_ms) VALUES (?, ?) \
                 ON CONFLICT(thread_id) DO UPDATE SET \
                 archived_at_ms = COALESCE(archived_at_ms, excluded.archived_at_ms)",
                [id(), now()],
                "archive thread",
            )
            .await
        }
        ThreadChange::Unarchive => {
            execute(
                transaction,
                "UPDATE thread_lifecycle SET archived_at_ms = NULL, history_at_ms = NULL \
                 WHERE thread_id = ?",
                [id()],
                "restore thread",
            )
            .await?;
            execute(
                transaction,
                "INSERT INTO thread_reads (thread_id, read_at_ms) VALUES (?, ?) \
                 ON CONFLICT(thread_id) DO UPDATE SET read_at_ms = MAX(read_at_ms, excluded.read_at_ms)",
                [id(), now()],
                "record restored thread as seen",
            )
            .await
        }
    }
}

async fn erase_thread(
    transaction: &DatabaseTransaction,
    thread: &ThreadId,
) -> Result<(), RepositoryError> {
    for sql in ERASE_STATEMENTS {
        execute(
            transaction,
            sql,
            [text(thread.as_str())],
            "erase archived thread",
        )
        .await?;
    }
    Ok(())
}

fn text(value: &str) -> Value {
    Value::String(Some(value.to_owned()))
}

fn statement<const N: usize>(sql: &str, values: [Value; N]) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
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
