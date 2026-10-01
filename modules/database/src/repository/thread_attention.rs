//! What each listed thread wants from its reader.

use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use artisan_domain::{ThreadAttention, ThreadSummary};

use super::{Repository, RepositoryError, database_error};

/// Per thread: whether an approval or question is open on a run that has
/// not settled, the lifecycle and settle time of the latest run, and when
/// the reader last had the thread open (0 when never). The thread ids bind
/// after `IN`.
const THREAD_ATTENTION_SQL: &str = r"
SELECT t.thread_id,
       EXISTS (SELECT 1 FROM pending_run_interactions AS p
               JOIN assistant_runs AS r ON r.run_id = p.run_id
               WHERE p.thread_id = t.thread_id AND p.state = 'requested'
                 AND r.lifecycle IN ('queued', 'launching', 'running', 'waiting', 'cancel_requested')),
       (SELECT r.lifecycle FROM assistant_runs AS r WHERE r.thread_id = t.thread_id
        ORDER BY r.created_at_ms DESC, r.run_id DESC LIMIT 1),
       (SELECT COALESCE(r.terminal_at_ms, r.updated_at_ms) FROM assistant_runs AS r
        WHERE r.thread_id = t.thread_id
        ORDER BY r.created_at_ms DESC, r.run_id DESC LIMIT 1),
       COALESCE((SELECT d.read_at_ms FROM thread_reads AS d WHERE d.thread_id = t.thread_id), 0)
FROM threads AS t
WHERE t.thread_id IN ";

impl Repository {
    /// Fills [`ThreadSummary::attention`] for stored thread rows.
    ///
    /// An open approval or question on an unsettled run is
    /// [`ThreadAttention::AwaitingAnswer`]; the caller that knows which runs
    /// the Forge really owns drops it for a run that is no longer live.
    /// Otherwise the latest run's outcome counts while it settled after the
    /// reader last had the thread open: completed is
    /// [`ThreadAttention::Finished`], failed or interrupted is
    /// [`ThreadAttention::Failed`]. A cancelled run wants nothing.
    pub(super) async fn project_thread_attention(
        &self,
        summaries: &mut [ThreadSummary],
    ) -> Result<(), RepositoryError> {
        if summaries.is_empty() {
            return Ok(());
        }
        let placeholders = vec!["?"; summaries.len()].join(", ");
        let rows = self
            .database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                format!("{THREAD_ATTENTION_SQL}({placeholders})"),
                summaries
                    .iter()
                    .map(|thread| Value::String(Some(thread.thread_id.as_str().to_owned()))),
            ))
            .await
            .map_err(|source| database_error("list thread attention", source))?;
        let mut attention = HashMap::with_capacity(rows.len());
        for row in &rows {
            let read = |source| database_error("read thread attention", source);
            let thread_id = row.try_get_by_index::<String>(0).map_err(read)?;
            let awaiting = row.try_get_by_index::<bool>(1).map_err(read)?;
            let lifecycle = row.try_get_by_index::<Option<String>>(2).map_err(read)?;
            let settled_at = row.try_get_by_index::<Option<i64>>(3).map_err(read)?;
            let read_at = row.try_get_by_index::<i64>(4).map_err(read)?;
            let unread = settled_at.is_some_and(|settled_at| settled_at > read_at);
            let wants = if awaiting {
                ThreadAttention::AwaitingAnswer
            } else {
                match lifecycle.as_deref() {
                    Some("completed") if unread => ThreadAttention::Finished,
                    Some("failed" | "interrupted") if unread => ThreadAttention::Failed,
                    _ => ThreadAttention::None,
                }
            };
            attention.insert(thread_id, wants);
        }
        for thread in summaries.iter_mut() {
            thread.attention = attention
                .get(thread.thread_id.as_str())
                .copied()
                .unwrap_or_default();
        }
        Ok(())
    }
}
