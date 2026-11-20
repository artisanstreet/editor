use crate::content::ContentError;
use crate::error::AtStep as _;
use crate::error::StorageError;
use artisan_api::{
    Envelope, Event, Item, ItemId, ItemsPage, ItemsQuery, Project, ProjectId, Run, RunId, Seq,
    Thread, ThreadId, WorkSpan,
};
use rusqlite::{Connection, OptionalExtension, Params, params};
use serde::{Serialize, de::DeserializeOwned};

type Result<T, E = StorageError> = std::result::Result<T, E>;

const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 500;
pub const WAITING_ITEMS: usize = 1000;
pub const MAX_SEARCH_BYTES: usize = 256;

pub(crate) fn to_body<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(StorageError::encode::<T>)
}

pub(crate) fn from_body<T: DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(StorageError::decode::<T>)
}

pub(crate) fn from_value<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(StorageError::decode::<T>)
}

pub(crate) fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(StorageError::encode::<T>)
}

pub(crate) fn tag<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(tag)) => tag,
        Ok(other) => unreachable!("invariant: stored statuses are unit variants, not {other}"),
        Err(error) => unreachable!("invariant: stored statuses always encode: {error}"),
    }
}

pub(crate) fn untag<T: DeserializeOwned>(tag: String) -> Result<T> {
    from_value(serde_json::Value::String(tag))
}

fn one<T: DeserializeOwned>(
    conn: &Connection,
    step: &'static str,
    sql: &str,
    params: impl Params,
) -> Result<Option<T>> {
    conn.prepare_cached(sql)
        .at(step)?
        .query_row(params, |row| row.get::<_, String>(0))
        .optional()
        .at(step)?
        .map(|body| from_body(&body))
        .transpose()
}

pub(crate) fn many<T: DeserializeOwned>(
    conn: &Connection,
    step: &'static str,
    sql: &str,
    params: impl Params,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare_cached(sql).at(step)?;
    let rows = statement
        .query_map(params, |row| row.get::<_, String>(0))
        .at(step)?;
    rows.map(|body| from_body(&body.at(step)?)).collect()
}

pub(crate) fn max_seq(conn: &Connection) -> Result<Seq> {
    conn.query_row(include_str!("sql/state/max_seq.sql"), [], |row| row.get(0))
        .at("state/max_seq.sql")
}

pub(crate) fn project(conn: &Connection, id: &ProjectId) -> Result<Option<Project>> {
    one(
        conn,
        "state/project.sql",
        include_str!("sql/state/project.sql"),
        [id.as_str()],
    )
}

pub(crate) fn thread(conn: &Connection, id: &ThreadId) -> Result<Option<Thread>> {
    one(
        conn,
        "state/thread.sql",
        include_str!("sql/state/thread.sql"),
        [id.as_str()],
    )
}

fn item_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}
pub(crate) fn items(
    conn: &Connection,
    step: &'static str,
    sql: &str,
    params: impl Params,
) -> Result<Vec<Item>> {
    conn.prepare_cached(sql)
        .at(step)?
        .query_map(params, item_row)
        .at(step)?
        .collect::<rusqlite::Result<_>>()
        .at(step)
}
pub(crate) fn item(conn: &Connection, id: &ItemId) -> Result<Option<Item>> {
    conn.prepare_cached(include_str!("sql/items/by_id.sql"))
        .at("items/by_id.sql")?
        .query_row([id.as_str()], item_row)
        .optional()
        .at("items/by_id.sql")
}
pub(crate) fn item_by_native(
    conn: &Connection,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    native_id: &str,
) -> Result<Option<Item>> {
    conn.prepare_cached(include_str!("sql/items/by_native.sql"))
        .at("items/by_native.sql")?
        .query_row(
            params![thread_id.as_str(), native_id, run_id.map(RunId::as_str)],
            item_row,
        )
        .optional()
        .at("items/by_native.sql")
}

pub(crate) fn agent_by_address(
    conn: &Connection,
    thread_id: &ThreadId,
    address: &str,
) -> Result<Option<ItemId>> {
    conn.prepare_cached(include_str!("sql/items/agent_by_address.sql"))
        .at("items/agent_by_address.sql")?
        .query_row(params![thread_id.as_str(), address], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map(|id| id.map(ItemId))
        .at("items/agent_by_address.sql")
}

pub(crate) fn agent_labels(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<String>> {
    let mut statement = conn
        .prepare_cached(include_str!("sql/items/agent_labels.sql"))
        .at("items/agent_labels.sql")?;
    let labels = statement
        .query_map([thread_id.as_str()], |row| row.get::<_, String>(0))
        .at("items/agent_labels.sql")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .at("items/agent_labels.sql")?;
    Ok(labels)
}

pub(crate) fn items_awaiting_user(conn: &Connection) -> Result<Vec<artisan_api::PendingItem>> {
    let mut statement = conn
        .prepare_cached(include_str!("sql/pending/awaiting_user.sql"))
        .at("pending/awaiting_user.sql")?;
    statement
        .query_map([WAITING_ITEMS], |row| {
            Ok(artisan_api::PendingItem {
                id: ItemId(row.get(0)?),
                thread_id: ThreadId(row.get(1)?),
            })
        })
        .at("pending/awaiting_user.sql")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .at("pending/awaiting_user.sql")
}

pub(crate) fn pending_threads(
    conn: &Connection,
    limit: usize,
    excluded: &[ThreadId],
    now: artisan_api::Timestamp,
) -> Result<Vec<ThreadId>> {
    let mut query = conn
        .prepare_cached(concat!(
            include_str!("sql/pending/runnable_inputs.sql"),
            include_str!("sql/pending/threads.sql")
        ))
        .at("pending/threads.sql")?;
    query
        .query_map(params![to_body(excluded)?, limit, now], |row| {
            row.get::<_, String>(0)
        })
        .at("pending/threads.sql")?
        .map(|row| row.map(ThreadId))
        .collect::<rusqlite::Result<_>>()
        .at("pending/threads.sql")
}

pub(crate) fn continue_request(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<Option<artisan_api::Continuation>> {
    let reason: Option<String> = conn
        .prepare_cached(include_str!("sql/pending/continue_request.sql"))
        .at("pending/continue_request.sql")?
        .query_row([thread.as_str()], |row| row.get(0))
        .optional()
        .at("pending/continue_request.sql")?;
    reason.map(untag).transpose()
}

pub(crate) fn has_runnable_input(conn: &Connection, thread_id: &ThreadId) -> Result<bool> {
    conn.prepare_cached(concat!(
        include_str!("sql/pending/runnable_inputs.sql"),
        include_str!("sql/pending/thread_has_input.sql")
    ))
    .at("pending/thread_has_input.sql")?
    .query_row([thread_id.as_str()], |row| row.get(0))
    .at("pending/thread_has_input.sql")
}

pub(crate) fn pending_inputs(
    conn: &Connection,
    thread_id: &ThreadId,
    blob_dir: &std::path::Path,
) -> Result<crate::PendingInputs> {
    const BATCH_ROWS: usize = 32;
    const BATCH_BYTES: usize = 4 * 1024 * 1024;
    let tx = conn.unchecked_transaction().at("begin a read snapshot")?;
    let mut query = tx
        .prepare_cached(include_str!("sql/pending/running_inputs.sql"))
        .at("pending/running_inputs.sql")?;
    let mut rows = query
        .query([thread_id.as_str()])
        .at("pending/running_inputs.sql")?;
    let mut items = Vec::with_capacity(BATCH_ROWS);
    let mut bytes = 0usize;
    let mut image_bytes = 0u64;
    while let Some(row) = rows.next().at("pending/running_inputs.sql")? {
        let body = row
            .get_ref(0)
            .at("pending/running_inputs.sql")?
            .as_str()
            .map_err(rusqlite::Error::from)
            .at("pending/running_inputs.sql")?;
        let item = item_row(row).at("pending/running_inputs.sql")?;
        let payload_bytes = if let artisan_api::ItemKind::UserMessage {
            text: artisan_api::TextContent::Stored(source),
            ..
        } = &item.kind
        {
            usize::try_from(source.bytes).unwrap_or(usize::MAX)
        } else {
            0
        };
        let next_bytes = body.len().saturating_add(payload_bytes);
        if !items.is_empty() && bytes.saturating_add(next_bytes) > BATCH_BYTES {
            break;
        }
        let next_images = if let artisan_api::ItemKind::UserMessage { images, .. } = &item.kind {
            crate::blobs::input_images(blob_dir, images)
                .map_or(128 * 1024 * 1024 + 1, |(_, bytes)| bytes)
        } else {
            0
        };
        if !items.is_empty() && image_bytes.saturating_add(next_images) > 128 * 1024 * 1024 {
            break;
        }
        bytes = bytes.saturating_add(next_bytes);
        image_bytes = image_bytes.saturating_add(next_images);
        items.push(item);
        if bytes >= BATCH_BYTES {
            break;
        }
    }
    let cutoff = items.iter().map(|item| item.seq).max().unwrap_or(0);
    Ok(crate::PendingInputs { items, cutoff })
}

pub(crate) fn run(conn: &Connection, id: &RunId) -> Result<Option<Run>> {
    one(
        conn,
        "runs/by_id.sql",
        include_str!("sql/runs/by_id.sql"),
        [id.as_str()],
    )
}

pub(crate) fn latest_run(conn: &Connection, thread_id: &ThreadId) -> Result<Option<Run>> {
    one(
        conn,
        "runs/latest.sql",
        include_str!("sql/runs/latest.sql"),
        [thread_id.as_str()],
    )
}

pub(crate) fn setting(conn: &Connection, key: &str) -> Result<Option<serde_json::Value>> {
    one(
        conn,
        "settings/value.sql",
        include_str!("sql/settings/value.sql"),
        [key],
    )
}

pub(crate) fn prompt_version(conn: &Connection) -> Result<u64> {
    let version: i64 = conn
        .query_row(include_str!("sql/settings/prompt_version.sql"), [], |row| {
            row.get(0)
        })
        .at("settings/prompt_version.sql")?;
    Ok(u64::try_from(version).unwrap_or(0))
}

pub(crate) fn draft(conn: &Connection, scope: &str) -> Result<artisan_api::DraftSnapshot> {
    let row: Option<(u64, String)> = conn
        .query_row(include_str!("sql/settings/draft.sql"), [scope], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .at("settings/draft.sql")?;
    match row {
        Some((revision, body)) => Ok(artisan_api::DraftSnapshot {
            revision,
            body: from_body(&body)?,
        }),
        None => Ok(artisan_api::DraftSnapshot::default()),
    }
}

pub(crate) fn items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
    scoped_items_page(conn, thread_id, query, None)
}

#[expect(
    clippy::option_option,
    reason = "None is every item, Some(None) the root transcript, Some(Some) one agent's work"
)]
pub(crate) fn scoped_items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
    parent: Option<Option<&ItemId>>,
) -> Result<ItemsPage> {
    const PAGE_BYTES: usize = 8 * 1024 * 1024;
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .at("begin a read snapshot")?;
    let limit = query
        .limit
        .map_or(DEFAULT_PAGE, |limit| {
            usize::try_from(limit).unwrap_or(MAX_PAGE)
        })
        .clamp(1, MAX_PAGE);
    let after = query.after.unwrap_or(0);
    let before = query
        .before
        .map_or(i64::MAX, |before| i64::try_from(before).unwrap_or(i64::MAX));
    let fetch = limit + 1;
    let (step, sql) = match (parent, query.after.is_some()) {
        (Some(Some(_)), true) => (
            "items/page/children_forward.sql",
            include_str!("sql/items/page/children_forward.sql"),
        ),
        (Some(Some(_)), false) => (
            "items/page/children_backward.sql",
            include_str!("sql/items/page/children_backward.sql"),
        ),
        (Some(None), true) => (
            "items/page/top_level_forward.sql",
            include_str!("sql/items/page/top_level_forward.sql"),
        ),
        (Some(None), false) => (
            "items/page/top_level_backward.sql",
            include_str!("sql/items/page/top_level_backward.sql"),
        ),
        (None, true) => (
            "items/page/all_forward.sql",
            include_str!("sql/items/page/all_forward.sql"),
        ),
        (None, false) => (
            "items/page/all_backward.sql",
            include_str!("sql/items/page/all_backward.sql"),
        ),
    };
    let mut statement = conn.prepare_cached(sql).at(step)?;
    let mut rows = if let Some(Some(parent)) = parent {
        statement
            .query(params![
                thread_id.as_str(),
                after,
                before,
                fetch,
                parent.as_str()
            ])
            .at(step)?
    } else {
        statement
            .query(params![thread_id.as_str(), after, before, fetch])
            .at(step)?
    };
    let mut items: Vec<Item> = Vec::new();
    let mut bytes = 0usize;
    let mut byte_limited = false;
    while let Some(row) = rows.next().at(step)? {
        let body: String = row.get(0).at(step)?;
        if bytes.saturating_add(body.len()) > PAGE_BYTES {
            byte_limited = true;
            break;
        }
        bytes = bytes.saturating_add(body.len());
        items.push(from_body(&body)?);
    }
    drop(rows);
    drop(statement);
    if query.after.is_none() {
        items.reverse();
    }
    let overflow = items.len() > limit;
    let more = overflow || byte_limited;
    if overflow {
        if query.after.is_some() {
            items.truncate(limit);
        } else {
            items.remove(0);
        }
    }
    let ids: std::collections::BTreeSet<_> = items
        .iter()
        .filter_map(|item| item.run_id.as_ref())
        .collect();
    let runs = many(
        conn,
        "runs/by_ids.sql",
        include_str!("sql/runs/by_ids.sql"),
        [to_body(&ids)?],
    )?;
    snapshot.commit().at("runs/by_ids.sql")?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work: Vec::new(),
    })
}

fn page_limit(query: &ItemsQuery) -> usize {
    query
        .limit
        .map_or(DEFAULT_PAGE, |limit| {
            usize::try_from(limit).unwrap_or(MAX_PAGE)
        })
        .clamp(1, MAX_PAGE)
}

fn page_bounds(query: &ItemsQuery) -> (i64, i64) {
    let after = query
        .after
        .map_or(0, |after| i64::try_from(after).unwrap_or(i64::MAX));
    let before = query
        .before
        .map_or(i64::MAX, |before| i64::try_from(before).unwrap_or(i64::MAX));
    (after, before)
}

const PAGE_BYTES: usize = 8 * 1024 * 1024;

struct Budget {
    items: usize,
    bytes: usize,
    admitted: usize,
}

impl Budget {
    const fn new(items: usize) -> Self {
        Self {
            items,
            bytes: PAGE_BYTES,
            admitted: 0,
        }
    }

    fn admit(&mut self, body: &str) -> bool {
        let fits = self.items > 0 && body.len() <= self.bytes;
        if !(fits || self.admitted == 0) {
            return false;
        }
        self.items = self.items.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(body.len());
        self.admitted += 1;
        true
    }
}

fn read_page(
    step: &'static str,
    rows: &mut rusqlite::Rows<'_>,
    limit: usize,
) -> Result<(Vec<Item>, bool)> {
    read_budgeted(step, rows, &mut Budget::new(limit))
}

fn read_budgeted(
    step: &'static str,
    rows: &mut rusqlite::Rows<'_>,
    budget: &mut Budget,
) -> Result<(Vec<Item>, bool)> {
    let mut items: Vec<Item> = Vec::new();
    while let Some(row) = rows.next().at(step)? {
        let body = row
            .get_ref(0)
            .at(step)?
            .as_str()
            .map_err(rusqlite::Error::from)
            .at(step)?;
        if !budget.admit(body) {
            return Ok((items, true));
        }
        items.push(from_body(body)?);
    }
    Ok((items, false))
}

fn page_runs(conn: &Connection, items: &[Item], work: &[WorkSpan]) -> Result<Vec<Run>> {
    let ids: std::collections::BTreeSet<_> = items
        .iter()
        .filter_map(|item| item.run_id.as_ref())
        .chain(work.iter().filter_map(|span| span.run_id.as_ref()))
        .collect();
    many(
        conn,
        "runs/by_ids.sql",
        include_str!("sql/runs/by_ids.sql"),
        [to_body(&ids)?],
    )
}

pub(crate) fn transcript_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .at("begin a read snapshot")?;
    let limit = page_limit(query);
    let (after, before) = page_bounds(query);
    let forward = query.after.is_some();
    let (step, sql) = if forward {
        (
            "transcript/rows_forward.sql",
            include_str!("sql/transcript/rows_forward.sql"),
        )
    } else {
        (
            "transcript/rows_backward.sql",
            include_str!("sql/transcript/rows_backward.sql"),
        )
    };
    let mut statement = conn.prepare_cached(sql).at(step)?;
    let mut rows = statement
        .query(params![thread_id.as_str(), after, before])
        .at(step)?;
    let mut budget = Budget::new(limit);
    let (mut items, more) = read_budgeted(step, &mut rows, &mut budget)?;
    drop(rows);
    drop(statement);
    if !forward {
        items.reverse();
    }
    let far = |end: Option<&Item>| {
        end.filter(|_| more)
            .map(|item| i64::try_from(item.position).unwrap_or(i64::MAX))
    };
    let (from, to) = if forward {
        (after, far(items.last()).unwrap_or(before))
    } else {
        (far(items.first()).unwrap_or(after), before)
    };
    let mut work = crate::work_spans::between(conn, thread_id, from, to)?;
    let runs = page_runs(conn, &items, &work)?;
    budget.items = DEFAULT_PAGE;
    for run in runs.iter().filter(|run| run.status().is_live()) {
        for span in work
            .iter_mut()
            .filter(|span| span.run_id.as_ref() == Some(&run.id))
        {
            let mut statement = conn
                .prepare_cached(include_str!("sql/transcript/work_rows.sql"))
                .at("transcript/work_rows.sql")?;
            let mut rows = statement
                .query(params![thread_id.as_str(), span.first, span.last])
                .at("transcript/work_rows.sql")?;
            let (newest, rest) = read_budgeted("transcript/work_rows.sql", &mut rows, &mut budget)?;
            drop(rows);
            drop(statement);
            let shipped = u32::try_from(newest.len()).unwrap_or(u32::MAX);
            if let Some(oldest) = newest.last().filter(|_| rest) {
                span.last = conn
                    .query_row(
                        include_str!("sql/transcript/last_work_position.sql"),
                        params![thread_id.as_str(), span.first, oldest.position],
                        |row| row.get(0),
                    )
                    .at("transcript/last_work_position.sql")?;
                span.count = span.count.saturating_sub(shipped);
            } else if rest {
                continue;
            } else {
                span.count = 0;
            }
            items.extend(newest);
        }
    }
    work.retain(|span| span.count > 0);
    items.sort_by_key(|item| item.position);
    snapshot.commit().at("transcript/last_work_position.sql")?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work,
    })
}

pub(crate) fn work_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .at("begin a read snapshot")?;
    let limit = page_limit(query);
    let (after, before) = page_bounds(query);
    let mut statement = conn
        .prepare_cached(include_str!("sql/transcript/work_page.sql"))
        .at("transcript/work_page.sql")?;
    let mut rows = statement
        .query(params![thread_id.as_str(), after, before])
        .at("transcript/work_page.sql")?;
    let (mut items, more) = read_page("transcript/work_page.sql", &mut rows, limit)?;
    drop(rows);
    drop(statement);
    items.reverse();
    let runs = page_runs(conn, &items, &[])?;
    snapshot.commit().at("transcript/work_page.sql")?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work: Vec::new(),
    })
}

pub(crate) fn changes_since(
    conn: &Connection,
    since: Seq,
    limit: usize,
    summary: bool,
    observed: Option<&ThreadId>,
) -> Result<Option<Vec<Envelope>>> {
    let mut statement = conn
        .prepare_cached(include_str!("sql/state/changes_since.sql"))
        .at("state/changes_since.sql")?;
    let rows = statement
        .query_map(
            params![
                since,
                limit.saturating_add(1),
                summary,
                observed.map(ThreadId::as_str)
            ],
            |row| {
                Ok((
                    row.get::<_, Seq>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .at("state/changes_since.sql")?;
    let mut envelopes = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let (seq, table, body, key) = row.at("state/changes_since.sql")?;
        let Some(body) = body else {
            return Ok(None);
        };
        bytes = bytes.saturating_add(body.len());
        if envelopes.len() == limit || bytes > 16 * 1024 * 1024 {
            return Ok(None);
        }
        let event = match table.as_str() {
            "project" => Event::Project {
                project: from_body(&body)?,
            },
            "thread" => Event::Thread {
                thread: from_body(&body)?,
            },
            "thread_removed" => Event::ThreadRemoved {
                thread_id: from_body(&body)?,
            },
            "item" => Event::Item {
                item: from_body(&body)?,
            },
            "item_state" => {
                #[derive(serde::Deserialize)]
                struct ItemState {
                    id: ItemId,
                    thread_id: ThreadId,
                    status: artisan_api::Status,
                }
                let state: ItemState = from_body(&body)?;
                Event::ItemState {
                    id: state.id,
                    thread_id: state.thread_id,
                    status: state.status,
                }
            }
            "run" => Event::Run {
                run: from_body(&body)?,
            },
            "setting" => Event::Setting {
                key: key.unwrap_or_else(|| {
                    panic!("invariant: the setting change at seq {seq} carries its key")
                }),
                value: from_body(&body)?,
            },
            other => {
                unreachable!("invariant: changes_since.sql names no {other} change (seq {seq})")
            }
        };
        envelopes.push(Envelope {
            seq: Some(seq),
            live_revision: 0,
            event,
        });
    }
    if envelopes.is_empty() && since > max_seq(conn)? {
        return Ok(None);
    }
    Ok(Some(envelopes))
}

pub(crate) fn inspector(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<artisan_api::InspectorSummary, ContentError> {
    use artisan_api::{InspectorSummary, InspectorTerminal, InspectorWorkflow};
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .at("begin a read snapshot")?;
    let revision = conn
        .query_row(
            include_str!("sql/inspector/max_seq.sql"),
            [thread.as_str()],
            |row| row.get(0),
        )
        .at("inspector/max_seq.sql")?;
    let terminals: Vec<InspectorTerminal> = many(
        conn,
        "inspector/terminals.sql",
        include_str!("sql/inspector/terminals.sql"),
        [thread.as_str()],
    )?;
    let agents = live_agents(conn, thread)?;
    let workflows: Vec<InspectorWorkflow> = many(
        conn,
        "inspector/workflows.sql",
        include_str!("sql/inspector/workflows.sql"),
        [thread.as_str()],
    )?;
    let plan = current_plan(conn, thread)?;
    let goal = current_goal(conn, thread)?;
    let questions = many(
        conn,
        "inspector/questions.sql",
        include_str!("sql/inspector/questions.sql"),
        [thread.as_str()],
    )?;
    let edits = conn
        .query_row(
            include_str!("sql/inspector/edit_totals.sql"),
            [thread.as_str()],
            |row| {
                Ok(artisan_api::EditTotals {
                    added: row.get(0)?,
                    removed: row.get(1)?,
                })
            },
        )
        .optional()
        .at("inspector/edit_totals.sql")?;
    let outline = outline(conn, thread)?;
    snapshot.commit().at("inspector/edit_totals.sql")?;
    Ok(InspectorSummary {
        goal,
        questions,
        revision,
        terminals,
        agents,
        workflows,
        plan,
        edits,
        outline,
    })
}

fn live_agents(conn: &Connection, thread: &ThreadId) -> Result<Vec<artisan_api::InspectorAgent>> {
    let mut agents: Vec<artisan_api::InspectorAgent> = many(
        conn,
        "inspector/agents.sql",
        include_str!("sql/inspector/agents.sql"),
        [thread.as_str()],
    )?;
    for agent in &mut agents {
        if agent.label.is_empty() {
            agent.label = artisan_api::fallback_agent_name(agent.id.as_str()).into();
        }
        if let Some(config) = &mut agent.config
            && let Some(model) = &agent.model
            && config.model.as_ref() != Some(model)
            && agent.thread.is_none()
        {
            config.model = Some(model.clone());
        }
    }
    Ok(agents)
}

fn goal_state(state: &str) -> Option<artisan_api::ThreadGoalState> {
    match state {
        "active" => Some(artisan_api::ThreadGoalState::Active),
        "paused" => Some(artisan_api::ThreadGoalState::Paused),
        "blocked" => Some(artisan_api::ThreadGoalState::Blocked),
        "usageLimited" => Some(artisan_api::ThreadGoalState::UsageLimited),
        "budgetLimited" => Some(artisan_api::ThreadGoalState::BudgetLimited),
        _ => None,
    }
}

fn text_prefix(conn: &Connection, text: &artisan_api::TextContent) -> Result<String, ContentError> {
    match text {
        artisan_api::TextContent::Inline(text) => Ok(text.clone()),
        artisan_api::TextContent::Stored(source) => crate::content::range(
            conn,
            &source.id,
            &artisan_api::ContentQuery {
                offset: 0,
                limit: Some(1024),
            },
        )?
        .map(|range| range.text)
        .ok_or_else(|| ContentError::Missing {
            content: source.id.clone(),
        }),
    }
}

fn current_goal(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<Option<artisan_api::ThreadGoal>, ContentError> {
    let Some((state, objective)) = conn
        .query_row(
            include_str!("sql/inspector/goal.sql"),
            [thread.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .at("inspector/goal.sql")?
    else {
        return Ok(None);
    };
    let Some(state) = goal_state(&state) else {
        return Ok(None);
    };
    let objective = from_body::<artisan_api::TextContent>(&objective)?;
    let prefix = text_prefix(conn, &objective)?;
    let mut summary: String = prefix.chars().take(160).collect();
    if objective.byte_len() > summary.len() as u64 {
        summary.push('…');
    }
    Ok(Some(artisan_api::ThreadGoal {
        objective,
        summary,
        state,
    }))
}

fn current_plan(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<artisan_api::Collection<artisan_api::PlanStep>, ContentError> {
    let Some((position, steps)) = conn
        .query_row(
            include_str!("sql/inspector/current_plan.sql"),
            [thread.as_str()],
            |row| Ok((row.get::<_, Seq>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .at("inspector/current_plan.sql")?
    else {
        return Ok(artisan_api::Collection::default());
    };
    let plan: artisan_api::Collection<artisan_api::PlanStep> = from_body(&steps)?;
    let asked_since: bool = conn
        .query_row(
            include_str!("sql/inspector/asked_since.sql"),
            params![thread.as_str(), position],
            |row| row.get(0),
        )
        .at("inspector/asked_since.sql")?;
    if !asked_since {
        return Ok(plan);
    }
    let finished = |steps: &[artisan_api::PlanStep]| {
        steps.iter().all(|step| {
            matches!(
                step.status,
                artisan_api::StepStatus::Done | artisan_api::StepStatus::Cancelled
            )
        })
    };
    let done = match plan.inline() {
        Some(steps) => finished(steps),
        None => finished(&crate::collection::resolve(conn, &plan)?),
    };
    Ok(if done {
        artisan_api::Collection::default()
    } else {
        plan
    })
}

fn outline(conn: &Connection, thread: &ThreadId) -> Result<Vec<artisan_api::OutlineEntry>> {
    let mut statement = conn
        .prepare_cached(include_str!("sql/inspector/outline.sql"))
        .at("inspector/outline.sql")?;
    let entries = statement
        .query_map([thread.as_str()], |row| {
            Ok(artisan_api::OutlineEntry {
                id: ItemId(row.get(0)?),
                position: row.get(1)?,
                label: row.get(2)?,
            })
        })
        .at("inspector/outline.sql")?
        .collect::<rusqlite::Result<_>>()
        .at("inspector/outline.sql")?;
    Ok(entries)
}

pub(crate) fn outline_entry(
    conn: &Connection,
    item: &Item,
) -> Result<Option<String>, ContentError> {
    match &item.kind {
        artisan_api::ItemKind::UserMessage {
            text,
            steer,
            from_agent: None,
            ..
        } if item.parent_id.is_none() && !(*steer && item.run_id.is_some()) => {
            Ok(Some(outline_label(&text_prefix(conn, text)?)))
        }
        artisan_api::ItemKind::UserMessage { .. }
        | artisan_api::ItemKind::AssistantMessage { .. }
        | artisan_api::ItemKind::Tool { .. }
        | artisan_api::ItemKind::Agent { .. }
        | artisan_api::ItemKind::Question { .. }
        | artisan_api::ItemKind::Plan { .. }
        | artisan_api::ItemKind::Image { .. }
        | artisan_api::ItemKind::Compaction { .. }
        | artisan_api::ItemKind::Notice { .. }
        | artisan_api::ItemKind::Error { .. } => Ok(None),
    }
}

fn outline_label(text: &str) -> String {
    let prefix: String = text.chars().take(1024).collect();
    let line = prefix
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Image");
    let mut label: String = line.chars().take(160).collect();
    if line.chars().nth(160).is_some() {
        label.push('…');
    }
    label
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadsPageFailure {
    #[error("the search is {bytes} bytes; the limit is 256")]
    SearchTooLong { bytes: usize },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

pub(crate) fn threads_page(
    conn: &Connection,
    query: &artisan_api::ThreadsQuery,
) -> Result<artisan_api::ThreadsPage, ThreadsPageFailure> {
    let limit = query.limit.unwrap_or(100).clamp(1, 200) as usize;
    let (newer, cursor, id) = match &query.window {
        artisan_api::ThreadWindow::Latest => (false, i64::MAX as u64, ""),
        artisan_api::ThreadWindow::Before(cursor) => (
            false,
            cursor.activity_seq.min(i64::MAX as u64),
            cursor.id.as_str(),
        ),
        artisan_api::ThreadWindow::After(cursor) => (
            true,
            cursor.activity_seq.min(i64::MAX as u64),
            cursor.id.as_str(),
        ),
    };
    let search = query.query.as_deref().unwrap_or("").trim();
    if search.len() > MAX_SEARCH_BYTES {
        return Err(ThreadsPageFailure::SearchTooLong {
            bytes: search.len(),
        });
    }
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let indexed = search.chars().count() >= 3;
    let (step, sql) = match (newer, indexed) {
        (true, true) => (
            "state/threads/newer_indexed.sql",
            include_str!("sql/state/threads/newer_indexed.sql"),
        ),
        (true, false) => (
            "state/threads/newer_like.sql",
            include_str!("sql/state/threads/newer_like.sql"),
        ),
        (false, true) => (
            "state/threads/older_indexed.sql",
            include_str!("sql/state/threads/older_indexed.sql"),
        ),
        (false, false) => (
            "state/threads/older_like.sql",
            include_str!("sql/state/threads/older_like.sql"),
        ),
    };
    let search = if indexed {
        format!("\"{}\"", search.replace('\"', "\"\""))
    } else {
        escaped
    };
    let mut threads: Vec<Thread> = many(
        conn,
        step,
        sql,
        params![query.archived, cursor, id, search, limit + 1],
    )?;
    let more = threads.len() > limit;
    threads.truncate(limit);
    if newer {
        threads.reverse();
    }
    let next = if more && !newer {
        threads.last().map(|thread| artisan_api::ThreadCursor {
            activity_seq: thread.activity_seq,
            id: thread.id.clone(),
        })
    } else {
        None
    };
    let ids = to_body(&threads.iter().map(|thread| &thread.id).collect::<Vec<_>>())?;
    let runs = many(
        conn,
        "state/latest_runs.sql",
        include_str!("sql/state/latest_runs.sql"),
        [ids],
    )?;
    Ok(artisan_api::ThreadsPage {
        threads,
        runs,
        next,
        newer: newer && more,
    })
}

#[cfg(test)]
#[path = "../tests/read/waiting_query.rs"]
mod waiting_query_tests;

#[cfg(test)]
#[path = "../tests/read/outline.rs"]
mod outline_tests;
