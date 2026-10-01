//! Scoped observation-history reads.
//!
//! A subscriber opens a thread on its newest turns and reads the rest on
//! demand, so a history read is scoped twice. By turn: only the turns the
//! subscriber holds. By kind: the work rows of a settled turn (its tool
//! calls, commands, file edits and searches) stay on the Forge until that
//! turn's section is opened, and only a count of them is sent. A turn that
//! is still running sends everything, because its section is open.

use artisan_domain::{
    EARLIER_TURN_MARKER_LABEL_MAX_BYTES, EARLIER_TURN_MARKERS_MAX, EarlierTurnMarker,
    EngineObservationEvent, HeldBackTurnWork, ItemId, RunId, TURN_WORK_PAGE_MAX_ROWS, ThreadId,
    TurnId, TurnOrdinal, UnixMillis,
};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, Value};

use crate::entities::observation_ledger;

use super::observation_ledger::ledger_event;
use super::run_observation::OBSERVATION_BATCH_MAX_OBSERVATIONS;
use super::{Repository, RepositoryError, corrupt_data, database_error};

/// Observation kinds that show only as rows inside a turn's work section.
const WORK_TAGS: &str = "('tool','terminal_activity','file','search','subagent_transcript')";

/// Turn lifecycles whose work section is closed: nothing more is arriving.
const SETTLED_TURN_LIFECYCLES: &str = "('completed','failed','cancelled','interrupted')";

/// Most payload bytes one page of a turn's work rows carries. A row is at
/// most 256 KiB, so a page always fits a transport frame.
const TURN_WORK_PAGE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Which part of a thread's observation history a subscriber holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationHistoryScope {
    /// Inclusive ordinal of the oldest turn in scope.
    pub floor: u64,
    /// Exclusive upper turn ordinal, or `None` through the newest turn.
    pub before: Option<u64>,
    /// Last delivery sequence in scope: rows after it are live and are
    /// delivered whole.
    pub tail: u64,
}

impl ObservationHistoryScope {
    fn bounds(&self) -> Option<(i64, i64, i64)> {
        let floor = i64::try_from(self.floor).ok()?;
        let before = self
            .before
            .map_or(i64::MAX, |before| i64::try_from(before).unwrap_or(i64::MAX));
        let tail = i64::try_from(self.tail).unwrap_or(i64::MAX);
        Some((floor, before, tail))
    }
}

impl Repository {
    /// The newest delivery sequence committed on `thread_id`, or zero.
    ///
    /// # Errors
    ///
    /// Returns a preserved database failure.
    pub async fn observation_history_tail(
        &self,
        thread_id: &ThreadId,
    ) -> Result<u64, RepositoryError> {
        let row = self
            .database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT COALESCE(MAX(delivery_sequence), 0) FROM observation_ledger \
                 WHERE thread_id = ?",
                [thread_id.as_str().to_owned().into()],
            ))
            .await
            .map_err(|source| database_error("read observation history tail", source))?;
        let tail = row
            .map(|row| row.try_get_by_index::<i64>(0))
            .transpose()
            .map_err(|source| database_error("read observation history tail", source))?
            .unwrap_or(0);
        u64::try_from(tail).map_err(|_| {
            corrupt_data(
                "observation_ledger",
                "delivery_sequence",
                "counter is negative",
            )
        })
    }

    /// Reads the rows of `scope` a subscriber receives while every settled
    /// section is closed: everything of a running turn, and everything but
    /// the work rows of a settled one.
    ///
    /// Returns at most `limit` events (capped at one batch) after
    /// `after_sequence`, in delivery order.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn read_scoped_observation_history(
        &self,
        thread_id: &ThreadId,
        after_sequence: u64,
        scope: &ObservationHistoryScope,
        limit: usize,
    ) -> Result<Vec<EngineObservationEvent>, RepositoryError> {
        let (Ok(after), Some((floor, before, tail))) =
            (i64::try_from(after_sequence), scope.bounds())
        else {
            return Ok(Vec::new());
        };
        if limit == 0 {
            return Ok(Vec::new());
        }
        let take = i64::try_from(limit.min(OBSERVATION_BATCH_MAX_OBSERVATIONS)).unwrap_or(i64::MAX);
        let rows = observation_ledger::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                format!(
                    "SELECT l.* FROM observation_ledger AS l \
                     JOIN conversation_turns AS t ON t.turn_id = l.turn_id \
                     WHERE l.thread_id = ? AND l.delivery_sequence > ? \
                       AND l.delivery_sequence <= ? AND t.ordinal >= ? AND t.ordinal < ? \
                       AND NOT (t.lifecycle IN {SETTLED_TURN_LIFECYCLES} \
                                AND l.observation_tag IN {WORK_TAGS}) \
                     ORDER BY l.delivery_sequence ASC LIMIT ?"
                ),
                [
                    Value::from(thread_id.as_str().to_owned()),
                    after.into(),
                    tail.into(),
                    floor.into(),
                    before.into(),
                    take.into(),
                ],
            ))
            .all(&self.database)
            .await
            .map_err(|source| database_error("read scoped observation history", source))?;
        rows.iter().map(ledger_event).collect()
    }

    /// Counts the work rows of `scope`'s settled turns that
    /// [`Self::read_scoped_observation_history`] leaves out, one entry per
    /// turn that has any, oldest turn first.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn read_held_back_turn_work(
        &self,
        thread_id: &ThreadId,
        scope: &ObservationHistoryScope,
    ) -> Result<Vec<HeldBackTurnWork>, RepositoryError> {
        let Some((floor, before, tail)) = scope.bounds() else {
            return Ok(Vec::new());
        };
        // SQLite takes the bare `run_id` and `committed_at_ms` from the row
        // that holds the one `MIN`: the turn's first held-back row.
        let rows = self
            .database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                format!(
                    "SELECT l.turn_id, COUNT(*), MIN(l.delivery_sequence), l.run_id, \
                            l.committed_at_ms \
                     FROM observation_ledger AS l \
                     JOIN conversation_turns AS t ON t.turn_id = l.turn_id \
                     WHERE l.thread_id = ? AND l.delivery_sequence <= ? \
                       AND t.ordinal >= ? AND t.ordinal < ? \
                       AND t.lifecycle IN {SETTLED_TURN_LIFECYCLES} \
                       AND l.observation_tag IN {WORK_TAGS} \
                     GROUP BY l.turn_id ORDER BY MIN(l.delivery_sequence) ASC"
                ),
                [
                    Value::from(thread_id.as_str().to_owned()),
                    tail.into(),
                    floor.into(),
                    before.into(),
                ],
            ))
            .await
            .map_err(|source| database_error("count held-back turn work", source))?;
        rows.iter()
            .map(|row| {
                let read = |source| database_error("read held-back turn work", source);
                let turn_id = TurnId::parse(row.try_get_by_index::<String>(0).map_err(read)?)
                    .map_err(|error| corrupt_data("observation_ledger", "turn_id", error))?;
                let count = row.try_get_by_index::<i64>(1).map_err(read)?;
                let first = row.try_get_by_index::<i64>(2).map_err(read)?;
                let run_id = RunId::parse(row.try_get_by_index::<String>(3).map_err(read)?)
                    .map_err(|error| corrupt_data("observation_ledger", "run_id", error))?;
                let committed_at = row.try_get_by_index::<i64>(4).map_err(read)?;
                Ok(HeldBackTurnWork {
                    turn_id,
                    run_id,
                    row_count: u32::try_from(count).unwrap_or(u32::MAX),
                    first_committed_at: UnixMillis::from_millis(committed_at),
                    first_delivery_sequence: u64::try_from(first).map_err(|_| {
                        corrupt_data(
                            "observation_ledger",
                            "delivery_sequence",
                            "delivery sequence must be positive",
                        )
                    })?,
                })
            })
            .collect()
    }

    /// Names the user messages of the turns before `before_turn_ordinal`,
    /// oldest first: what a turn navigator lists for turns that are not
    /// loaded.
    ///
    /// At most [`EARLIER_TURN_MARKERS_MAX`] messages are named, the newest
    /// ones, each with the start of its text. A message without text (images
    /// only) is left out: it has nothing to be listed by.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn read_earlier_turn_markers(
        &self,
        thread_id: &ThreadId,
        before_turn_ordinal: u64,
    ) -> Result<Vec<EarlierTurnMarker>, RepositoryError> {
        let before = i64::try_from(before_turn_ordinal).unwrap_or(i64::MAX);
        let limit = i64::try_from(EARLIER_TURN_MARKERS_MAX).unwrap_or(i64::MAX);
        // SQLite's `substr` counts characters of text, so the prefix ends on
        // a character boundary; the byte bound is applied below.
        let characters = i64::try_from(EARLIER_TURN_MARKER_LABEL_MAX_BYTES).unwrap_or(i64::MAX);
        let rows = self
            .database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT i.item_id, t.ordinal, substr(i.body, 1, ?) \
                 FROM conversation_items AS i \
                 JOIN conversation_turns AS t ON t.turn_id = i.turn_id \
                 WHERE i.thread_id = ? AND i.item_kind = 'user_message' \
                   AND t.ordinal < ? AND i.body <> '' \
                 ORDER BY i.ordinal DESC LIMIT ?",
                [
                    Value::from(characters),
                    thread_id.as_str().to_owned().into(),
                    before.into(),
                    limit.into(),
                ],
            ))
            .await
            .map_err(|source| database_error("read earlier turn markers", source))?;
        let mut markers = rows
            .iter()
            .map(|row| {
                let read = |source| database_error("read earlier turn marker", source);
                let item_id = ItemId::parse(row.try_get_by_index::<String>(0).map_err(read)?)
                    .map_err(|error| corrupt_data("conversation_items", "item_id", error))?;
                let ordinal = u64::try_from(row.try_get_by_index::<i64>(1).map_err(read)?)
                    .map_err(|_| {
                        corrupt_data("conversation_turns", "ordinal", "counter is negative")
                    })?;
                let mut label = row.try_get_by_index::<String>(2).map_err(read)?;
                let mut end = label.len().min(EARLIER_TURN_MARKER_LABEL_MAX_BYTES);
                while !label.is_char_boundary(end) {
                    end -= 1;
                }
                label.truncate(end);
                Ok(EarlierTurnMarker {
                    item_id,
                    turn_ordinal: TurnOrdinal::new(ordinal),
                    label,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        markers.reverse();
        Ok(markers)
    }

    /// Reads one page of a turn's work rows after `after_sequence`, in
    /// delivery order, with the sequence to continue after when more remain.
    ///
    /// A page holds at most [`TURN_WORK_PAGE_MAX_ROWS`] rows and stops early
    /// once their payloads pass a fixed byte budget, so it always fits one
    /// transport frame.
    ///
    /// # Errors
    ///
    /// Returns corrupt persisted data or a preserved database failure.
    pub async fn read_turn_work(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        after_sequence: u64,
    ) -> Result<(Vec<EngineObservationEvent>, Option<u64>), RepositoryError> {
        let Ok(after) = i64::try_from(after_sequence) else {
            return Ok((Vec::new(), None));
        };
        // One row beyond the page tells whether more remain.
        let take = i64::try_from(TURN_WORK_PAGE_MAX_ROWS + 1).unwrap_or(i64::MAX);
        let rows = observation_ledger::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                format!(
                    "SELECT l.* FROM observation_ledger AS l \
                     WHERE l.thread_id = ? AND l.turn_id = ? AND l.delivery_sequence > ? \
                       AND l.observation_tag IN {WORK_TAGS} \
                     ORDER BY l.delivery_sequence ASC LIMIT ?"
                ),
                [
                    Value::from(thread_id.as_str().to_owned()),
                    turn_id.as_str().to_owned().into(),
                    after.into(),
                    take.into(),
                ],
            ))
            .all(&self.database)
            .await
            .map_err(|source| database_error("read turn work", source))?;
        let mut bytes = 0_usize;
        let mut page = Vec::new();
        for row in &rows {
            if page.len() == TURN_WORK_PAGE_MAX_ROWS
                || (!page.is_empty() && bytes >= TURN_WORK_PAGE_MAX_BYTES)
            {
                break;
            }
            bytes = bytes.saturating_add(row.observation_bytes.as_slice().len());
            page.push(ledger_event(row)?);
        }
        let next = (page.len() < rows.len())
            .then(|| {
                page.last()
                    .and_then(|event| event.attribution.as_ref())
                    .map(|attribution| attribution.delivery_sequence)
            })
            .flatten();
        Ok((page, next))
    }
}
