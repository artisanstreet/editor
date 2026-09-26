//! The cross-project listing of recently active threads and its change
//! fingerprint.

use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement, Value,
};

use artisan_domain::ThreadSummary;

use crate::entities;

use super::project_threads::thread_summary;
use super::{Repository, RepositoryError, corrupt_data, database_error};

/// Saved threads (assistant text started) by last activity, newest first.
const RECENT_THREAD_IDS_SQL: &str = r"
SELECT t.thread_id,
       COALESCE((SELECT MAX(m.accepted_at_ms) FROM messages AS m WHERE m.thread_id = t.thread_id),
                t.updated_at_ms) AS activity
FROM threads AS t
WHERE EXISTS (SELECT 1 FROM conversation_items AS c
              WHERE c.thread_id = t.thread_id AND c.item_kind = 'assistant_message'
                AND c.body <> '')
ORDER BY activity DESC, t.thread_id ASC
LIMIT ?
";

/// Cheap aggregate over every input of the recent-threads listing: the
/// project and thread catalogs, titles, messages, started threads, and live
/// runs. Any change the listing shows changes at least one component.
const RECENT_THREADS_FINGERPRINT_SQL: &str = r"
SELECT (SELECT COUNT(*) FROM attached_projects),
       (SELECT COUNT(*) FROM threads),
       (SELECT COALESCE(SUM(length(title) * 131 + unicode(title) + updated_at_ms % 1000003), 0)
          FROM threads),
       (SELECT COUNT(*) FROM messages),
       (SELECT COALESCE(MAX(accepted_at_ms), 0) FROM messages),
       (SELECT COUNT(*) FROM threads AS t
          WHERE EXISTS (SELECT 1 FROM conversation_items AS c
                        WHERE c.thread_id = t.thread_id AND c.item_kind = 'assistant_message'
                          AND c.body <> '')),
       (SELECT COUNT(*) FROM assistant_runs
          WHERE lifecycle IN ('queued', 'launching', 'running', 'waiting', 'cancel_requested')),
       (SELECT COALESCE(SUM(length(display_name)), 0) FROM attached_projects)
";

/// Opaque summary of the recent-threads inputs; equal fingerprints mean the
/// listing has not changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecentThreadsFingerprint([i64; 8]);

impl Repository {
    /// Lists the most recently active saved threads across every project,
    /// newest first: by the latest message, else by the last update. Drafts
    /// (no assistant text yet) are not listed. Ties order by thread id.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn list_recent_threads(
        &self,
        limit: usize,
    ) -> Result<Vec<ThreadSummary>, RepositoryError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let ids = self
            .database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                RECENT_THREAD_IDS_SQL,
                [Value::BigInt(Some(limit))],
            ))
            .await
            .map_err(|source| database_error("list recent threads", source))?
            .iter()
            .map(|row| {
                row.try_get_by_index::<String>(0)
                    .map_err(|source| database_error("read recent thread id", source))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = entities::thread::Entity::find()
            .filter(entities::thread::Column::ThreadId.is_in(ids.iter().map(String::as_str)))
            .all(&self.database)
            .await
            .map_err(|source| database_error("read recent threads", source))?;
        let mut summaries = rows
            .into_iter()
            .map(thread_summary)
            .collect::<Result<Vec<_>, _>>()?;
        self.project_listed_threads(&mut summaries).await?;
        summaries.sort_by(|left, right| {
            let activity =
                |thread: &ThreadSummary| thread.last_message_at.unwrap_or(thread.updated_at);
            activity(right)
                .cmp(&activity(left))
                .then_with(|| left.thread_id.as_str().cmp(right.thread_id.as_str()))
        });
        Ok(summaries)
    }

    /// Summarizes everything the recent-threads listing shows, so delivery
    /// re-reads and pushes it only when it changed.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn recent_threads_fingerprint(
        &self,
    ) -> Result<RecentThreadsFingerprint, RepositoryError> {
        let row = self
            .database
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                RECENT_THREADS_FINGERPRINT_SQL,
            ))
            .await
            .map_err(|source| database_error("read recent threads fingerprint", source))?
            .ok_or_else(|| {
                corrupt_data(
                    "threads",
                    "count",
                    "recent threads fingerprint returned no row",
                )
            })?;
        let mut parts = [0_i64; 8];
        for (index, part) in parts.iter_mut().enumerate() {
            *part = row
                .try_get_by_index::<i64>(index)
                .map_err(|source| database_error("read recent threads fingerprint", source))?;
        }
        Ok(RecentThreadsFingerprint(parts))
    }
}
