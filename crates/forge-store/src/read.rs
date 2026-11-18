use crate::content::ContentError;
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

pub fn to_body<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(StorageError::encode::<T>)
}

pub fn from_body<T: DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(StorageError::decode::<T>)
}

pub fn from_value<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(StorageError::decode::<T>)
}

pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(StorageError::encode::<T>)
}

pub fn tag<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(tag)) => tag,
        Ok(other) => unreachable!("invariant: stored statuses are unit variants, not {other}"),
        Err(error) => unreachable!("invariant: stored statuses always encode: {error}"),
    }
}

pub fn untag<T: DeserializeOwned>(tag: String) -> Result<T> {
    from_value(serde_json::Value::String(tag))
}

fn one<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Option<T>> {
    conn.prepare_cached(sql)?
        .query_row(params, |row| row.get::<_, String>(0))
        .optional()?
        .map(|body| from_body(&body))
        .transpose()
}

pub fn many<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare_cached(sql)?;
    let rows = statement.query_map(params, |row| row.get::<_, String>(0))?;
    rows.map(|body| from_body(&body?)).collect()
}

pub fn max_seq(conn: &Connection) -> Result<Seq> {
    conn.query_row(
        "SELECT coalesce(max(seq), 0) FROM (
            SELECT max(seq) AS seq FROM projects UNION ALL
            SELECT max(seq) FROM threads UNION ALL
            SELECT max(seq) FROM items UNION ALL
            SELECT max(seq) FROM runs UNION ALL
            SELECT max(seq) FROM removed_threads UNION ALL
            SELECT max(seq) FROM kv)",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn project(conn: &Connection, id: &ProjectId) -> Result<Option<Project>> {
    one(
        conn,
        "SELECT body FROM projects WHERE id = ?1",
        [id.as_str()],
    )
}

pub fn thread(conn: &Connection, id: &ThreadId) -> Result<Option<Thread>> {
    one(
        conn,
        "SELECT body FROM threads WHERE id = ?1",
        [id.as_str()],
    )
}

fn item_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}
pub fn items(conn: &Connection, sql: &str, params: impl Params) -> Result<Vec<Item>> {
    conn.prepare_cached(sql)?
        .query_map(params, item_row)?
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
}
pub fn item(conn: &Connection, id: &ItemId) -> Result<Option<Item>> {
    conn.prepare_cached("SELECT body FROM items WHERE id=?1")?
        .query_row([id.as_str()], item_row)
        .optional()
        .map_err(Into::into)
}
pub fn item_by_native(
    conn: &Connection,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    native_id: &str,
) -> Result<Option<Item>> {
    conn.prepare_cached("SELECT body FROM items WHERE thread_id=?1 AND native_id=?2 AND (?3 IS NULL OR run_id=?3) ORDER BY position DESC LIMIT 1")?
        .query_row(params![thread_id.as_str(),native_id,run_id.map(RunId::as_str)],item_row).optional().map_err(Into::into)
}

pub fn agent_by_address(
    conn: &Connection,
    thread_id: &ThreadId,
    address: &str,
) -> Result<Option<ItemId>> {
    conn.prepare_cached(
        "SELECT id FROM items WHERE thread_id=?1 AND kind_type='agent'
         AND json_extract(body,'$.kind.agent_id')=?2 ORDER BY position DESC LIMIT 1",
    )?
    .query_row(params![thread_id.as_str(), address], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .map(|id| id.map(ItemId))
    .map_err(Into::into)
}

pub fn agent_labels(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<String>> {
    let mut statement = conn.prepare_cached(
        "SELECT json_extract(body, '$.kind.label') FROM items
         WHERE thread_id = ?1 AND kind_type = 'agent'
           AND json_extract(body, '$.kind.label') IS NOT NULL",
    )?;
    let labels = statement
        .query_map([thread_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(labels)
}

pub fn items_awaiting_user(conn: &Connection) -> Result<Vec<artisan_api::PendingItem>> {
    let mut statement = conn.prepare_cached(include_str!("sql/pending/awaiting_user.sql"))?;
    statement
        .query_map([WAITING_ITEMS], |row| {
            Ok(artisan_api::PendingItem {
                id: ItemId(row.get(0)?),
                thread_id: ThreadId(row.get(1)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn pending_threads(
    conn: &Connection,
    limit: usize,
    excluded: &[ThreadId],
) -> Result<Vec<ThreadId>> {
    let mut query = conn.prepare_cached(concat!(
        include_str!("sql/pending/runnable_inputs.sql"),
        include_str!("sql/pending/threads.sql")
    ))?;
    Ok(query
        .query_map(
            params![to_body(excluded)?, limit, artisan_api::now()],
            |row| row.get::<_, String>(0),
        )?
        .map(|row| row.map(ThreadId))
        .collect::<rusqlite::Result<_>>()?)
}

pub fn continue_request(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<Option<artisan_api::Continuation>> {
    let reason: Option<String> = conn
        .prepare_cached(include_str!("sql/pending/continue_request.sql"))?
        .query_row([thread.as_str()], |row| row.get(0))
        .optional()?;
    reason.map(untag).transpose()
}

pub fn has_runnable_input(conn: &Connection, thread_id: &ThreadId) -> Result<bool> {
    Ok(conn
        .prepare_cached(concat!(
            include_str!("sql/pending/runnable_inputs.sql"),
            include_str!("sql/pending/thread_has_input.sql")
        ))?
        .query_row([thread_id.as_str()], |row| row.get(0))?)
}

pub fn pending_inputs(
    conn: &Connection,
    thread_id: &ThreadId,
    blob_dir: &std::path::Path,
) -> Result<crate::PendingInputs> {
    const BATCH_ROWS: usize = 32;
    const BATCH_BYTES: usize = 4 * 1024 * 1024;
    let tx = conn.unchecked_transaction()?;
    let mut query = tx.prepare_cached("SELECT body FROM items WHERE thread_id=?1 AND status='running' AND kind_type='user_message' ORDER BY position LIMIT 32")?;
    let mut rows = query.query([thread_id.as_str()])?;
    let mut items = Vec::with_capacity(BATCH_ROWS);
    let mut bytes = 0usize;
    let mut image_bytes = 0u64;
    while let Some(row) = rows.next()? {
        let body = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
        let item = item_row(row)?;
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

pub fn run(conn: &Connection, id: &RunId) -> Result<Option<Run>> {
    one(conn, "SELECT body FROM runs WHERE id = ?1", [id.as_str()])
}

pub fn latest_run(conn: &Connection, thread_id: &ThreadId) -> Result<Option<Run>> {
    one(
        conn,
        "SELECT body FROM runs WHERE thread_id = ?1 ORDER BY rowid DESC LIMIT 1",
        [thread_id.as_str()],
    )
}

pub fn setting(conn: &Connection, key: &str) -> Result<Option<serde_json::Value>> {
    one(conn, "SELECT value FROM kv WHERE key = ?1", [key])
}

pub fn prompt_version(conn: &Connection) -> Result<u64> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM prompt_versions",
        [],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(version).unwrap_or(0))
}

pub fn draft(conn: &Connection, scope: &str) -> Result<artisan_api::DraftSnapshot> {
    let row: Option<(u64, String)> = conn
        .query_row(
            "SELECT revision, body FROM drafts WHERE scope = ?1",
            [scope],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        Some((revision, body)) => Ok(artisan_api::DraftSnapshot {
            revision,
            body: from_body(&body)?,
        }),
        None => Ok(artisan_api::DraftSnapshot::default()),
    }
}

pub fn items_page(
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
pub fn scoped_items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
    parent: Option<Option<&ItemId>>,
) -> Result<ItemsPage> {
    const PAGE_BYTES: usize = 8 * 1024 * 1024;
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
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
    let order = if query.after.is_some() { "ASC" } else { "DESC" };
    let sql = match parent {
        Some(Some(_)) => format!("SELECT body FROM (
            SELECT body,position FROM items WHERE thread_id=?1 AND parent_id=?5 AND position>?2 AND position<?3
            UNION ALL
            SELECT body,position FROM items WHERE thread_id=?1 AND parent_id IS NULL
                AND json_extract(body,'$.kind.tool.kind')='agent_message' AND json_extract(body,'$.kind.tool.target')=?5 AND position>?2 AND position<?3
            ) ORDER BY position {order} LIMIT ?4"),
        Some(None) => format!("SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL AND position>?2 AND position<?3 ORDER BY position {order} LIMIT ?4"),
        None => format!("SELECT body FROM items WHERE thread_id=?1 AND position>?2 AND position<?3 ORDER BY position {order} LIMIT ?4"),
    };
    let mut statement = conn.prepare_cached(&sql)?;
    let mut rows = if let Some(Some(parent)) = parent {
        statement.query(params![
            thread_id.as_str(),
            after,
            before,
            fetch,
            parent.as_str()
        ])?
    } else {
        statement.query(params![thread_id.as_str(), after, before, fetch])?
    };
    let mut items: Vec<Item> = Vec::new();
    let mut bytes = 0usize;
    let mut byte_limited = false;
    while let Some(row) = rows.next()? {
        let body: String = row.get(0)?;
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
        "SELECT body FROM runs WHERE id IN (SELECT value FROM json_each(?1))",
        [to_body(&ids)?],
    )?;
    snapshot.commit()?;
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

fn read_page(rows: &mut rusqlite::Rows<'_>, limit: usize) -> Result<(Vec<Item>, bool)> {
    const PAGE_BYTES: usize = 8 * 1024 * 1024;
    let mut items: Vec<Item> = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next()? {
        let body: String = row.get(0)?;
        if items.len() == limit
            || (!items.is_empty() && bytes.saturating_add(body.len()) > PAGE_BYTES)
        {
            return Ok((items, true));
        }
        bytes = bytes.saturating_add(body.len());
        items.push(from_body(&body)?);
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
        "SELECT body FROM runs WHERE id IN (SELECT value FROM json_each(?1))",
        [to_body(&ids)?],
    )
}

fn work_spans(
    conn: &Connection,
    thread_id: &ThreadId,
    after: i64,
    before: i64,
) -> Result<Vec<WorkSpan>> {
    let mut statement = conn.prepare_cached(
        "SELECT position,run_id,work,kind_type FROM items
         WHERE thread_id=?1 AND parent_id IS NULL AND position>?2 AND position<?3
         ORDER BY position",
    )?;
    let mut rows = statement.query(params![thread_id.as_str(), after, before])?;
    let mut spans: Vec<WorkSpan> = Vec::new();
    let mut open = false;
    while let Some(row) = rows.next()? {
        let position: Seq = row.get(0)?;
        let run_id: Option<String> = row.get(1)?;
        let run_id = run_id.map(RunId::from);
        if !row.get::<_, bool>(2)? {
            open = false;
            continue;
        }
        let worked = matches!(
            row.get::<_, Option<String>>(3)?.as_deref(),
            Some("tool" | "agent" | "plan")
        );
        match spans.last_mut() {
            Some(span) if open && span.run_id == run_id => {
                span.last = position;
                span.count += 1;
                span.worked |= worked;
            }
            _ => {
                spans.push(WorkSpan {
                    run_id,
                    first: position,
                    last: position,
                    count: 1,
                    worked,
                });
                open = true;
            }
        }
    }
    Ok(spans)
}

pub fn transcript_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
    let limit = page_limit(query);
    let (after, before) = page_bounds(query);
    let forward = query.after.is_some();
    let order = if forward { "ASC" } else { "DESC" };
    let mut statement = conn.prepare_cached(&format!(
        "SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL AND work=0
         AND position>?2 AND position<?3 ORDER BY position {order}"
    ))?;
    let mut rows = statement.query(params![thread_id.as_str(), after, before])?;
    let (mut items, more) = read_page(&mut rows, limit)?;
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
    let mut work = work_spans(conn, thread_id, from, to)?;
    let runs = page_runs(conn, &items, &work)?;
    for run in runs.iter().filter(|run| run.status().is_live()) {
        for span in work
            .iter_mut()
            .filter(|span| span.run_id.as_ref() == Some(&run.id))
        {
            let mut statement = conn.prepare_cached(
                "SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL AND work=1
                 AND position>=?2 AND position<=?3 ORDER BY position DESC",
            )?;
            let mut rows = statement.query(params![thread_id.as_str(), span.first, span.last])?;
            let (newest, rest) = read_page(&mut rows, DEFAULT_PAGE)?;
            drop(rows);
            drop(statement);
            let shipped = u32::try_from(newest.len()).unwrap_or(u32::MAX);
            if let Some(oldest) = newest.last().filter(|_| rest) {
                span.last = conn.query_row(
                    "SELECT max(position) FROM items WHERE thread_id=?1 AND parent_id IS NULL
                     AND work=1 AND position>=?2 AND position<?3",
                    params![thread_id.as_str(), span.first, oldest.position],
                    |row| row.get(0),
                )?;
                span.count = span.count.saturating_sub(shipped);
            } else {
                span.count = 0;
            }
            items.extend(newest);
        }
    }
    work.retain(|span| span.count > 0);
    items.sort_by_key(|item| item.position);
    snapshot.commit()?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work,
    })
}

pub fn work_page(conn: &Connection, thread_id: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
    let limit = page_limit(query);
    let (after, before) = page_bounds(query);
    let mut statement = conn.prepare_cached(
        "SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL AND work=1
         AND position>?2 AND position<?3 ORDER BY position DESC",
    )?;
    let mut rows = statement.query(params![thread_id.as_str(), after, before])?;
    let (mut items, more) = read_page(&mut rows, limit)?;
    drop(rows);
    drop(statement);
    items.reverse();
    let runs = page_runs(conn, &items, &[])?;
    snapshot.commit()?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work: Vec::new(),
    })
}

pub fn changes_since(
    conn: &Connection,
    since: Seq,
    limit: usize,
    summary: bool,
    observed: Option<&ThreadId>,
) -> Result<Option<Vec<Envelope>>> {
    let mut statement = conn.prepare_cached(
        "SELECT seq, kind, CASE WHEN length(CAST(body AS BLOB)) <= 16777216 THEN body END, key FROM (
         SELECT seq, 'project' AS kind, body, NULL AS key FROM projects WHERE seq > ?1
         UNION ALL SELECT seq, 'thread', body, NULL FROM threads WHERE seq > ?1
         UNION ALL SELECT seq, 'thread_removed', json_quote(id), NULL FROM removed_threads WHERE seq > ?1
         UNION ALL SELECT seq,
             CASE WHEN ?3 AND (?4 IS NULL OR thread_id != ?4) THEN 'item_state' ELSE 'item' END,
             CASE WHEN ?3 AND (?4 IS NULL OR thread_id != ?4)
                 THEN json_object('id',id,'thread_id',thread_id,'status',status) ELSE body END,
             NULL FROM items WHERE seq > ?1
         UNION ALL SELECT seq, 'run', body, NULL FROM runs WHERE seq > ?1
         UNION ALL SELECT seq, 'setting', value, key FROM kv WHERE seq > ?1)
         ORDER BY 1 LIMIT ?2",
    )?;
    let rows = statement.query_map(
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
    )?;
    let mut envelopes = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let (seq, table, body, key) = row?;
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
            _ => Event::Setting {
                key: key.unwrap_or_default(),
                value: from_body(&body)?,
            },
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

pub fn inspector(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<artisan_api::InspectorSummary, ContentError> {
    use artisan_api::{InspectorSummary, InspectorTerminal, InspectorWorkflow};
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
    let revision = conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM (SELECT seq FROM items WHERE thread_id=?1 UNION ALL SELECT seq FROM runs WHERE thread_id=?1)",
        [thread.as_str()],
        |row| row.get(0),
    )?;
    let terminals: Vec<InspectorTerminal> = many(
        conn,
        include_str!("sql/inspector/terminals.sql"),
        [thread.as_str()],
    )?;
    let agents = live_agents(conn, thread)?;
    let workflows: Vec<InspectorWorkflow> = many(
        conn,
        include_str!("sql/inspector/workflows.sql"),
        [thread.as_str()],
    )?;
    let plan = current_plan(conn, thread)?;
    let goal = current_goal(conn, thread)?;
    let questions = many(
        conn,
        include_str!("sql/inspector/questions.sql"),
        [thread.as_str()],
    )?;
    let edits = conn
        .query_row(
            "SELECT added,removed FROM thread_edit_totals WHERE thread_id=?1 AND operations>0",
            [thread.as_str()],
            |row| {
                Ok(artisan_api::EditTotals {
                    added: row.get(0)?,
                    removed: row.get(1)?,
                })
            },
        )
        .optional()?;
    let outline = outline(conn, thread)?;
    snapshot.commit()?;
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
        .optional()?
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
            "SELECT position,json_extract(body,'$.kind.steps') FROM items
             WHERE thread_id=?1 AND kind_type='plan'
                AND parent_id IS NULL ORDER BY position DESC LIMIT 1",
            [thread.as_str()],
            |row| Ok((row.get::<_, Seq>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Ok(artisan_api::Collection::default());
    };
    let plan: artisan_api::Collection<artisan_api::PlanStep> = from_body(&steps)?;
    let asked_since: bool = conn.query_row(
        include_str!("sql/inspector/asked_since.sql"),
        params![thread.as_str(), position],
        |row| row.get(0),
    )?;
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

fn outline(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<Vec<artisan_api::OutlineEntry>, ContentError> {
    let mut statement = conn.prepare_cached(include_str!("sql/inspector/outline.sql"))?;
    let rows = statement.query_map([thread.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Seq>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (id, position, text) = row?;
        let text = from_body::<artisan_api::TextContent>(&text)?;
        Ok(artisan_api::OutlineEntry {
            id: ItemId(id),
            position,
            label: outline_label(&text_prefix(conn, &text)?),
        })
    })
    .collect()
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

impl From<rusqlite::Error> for ThreadsPageFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

pub fn threads_page(
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
    if search.len() > 256 {
        return Err(ThreadsPageFailure::SearchTooLong {
            bytes: search.len(),
        });
    }
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let (compare, order) = if newer { (">", "ASC") } else { ("<", "DESC") };
    let base = format!(
        "SELECT t.body FROM threads t JOIN projects p ON p.id=json_extract(t.body,'$.project_id')
         WHERE COALESCE(json_extract(t.body,'$.archived'),0)=?1
          AND COALESCE(json_extract(p.body,'$.removed'),0)=0
          AND (json_extract(t.body,'$.activity_seq'),t.id){compare}(?2,?3)"
    );
    let search_clause = if search.chars().count() >= 3 {
        " AND t.rowid IN (SELECT rowid FROM threads_search WHERE threads_search MATCH ?4)"
    } else {
        " AND (?4='' OR (COALESCE(json_extract(t.body,'$.title'),'') || ' ' || json_extract(p.body,'$.name')) LIKE '%' || ?4 || '%' ESCAPE '\\')"
    };
    let search = if search.chars().count() >= 3 {
        format!("\"{}\"", search.replace('\"', "\"\""))
    } else {
        escaped
    };
    let sql = format!(
        "{base}{search_clause} ORDER BY json_extract(t.body,'$.activity_seq') {order},t.id {order} LIMIT ?5"
    );
    let mut threads: Vec<Thread> = many(
        conn,
        &sql,
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
    let runs = many(conn,
        "SELECT body FROM runs r WHERE r.thread_id IN (SELECT value FROM json_each(?1))
          AND r.rowid=(SELECT rowid FROM runs WHERE thread_id=r.thread_id ORDER BY rowid DESC LIMIT 1)", [ids])?;
    Ok(artisan_api::ThreadsPage {
        threads,
        runs,
        next,
        newer: newer && more,
    })
}

#[cfg(test)]
mod waiting_query_tests {
    #[test]
    fn waiting_query_uses_its_ordered_partial_index_with_one_hundred_thousand_historical_items() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        conn.execute_batch("WITH RECURSIVE history(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM history WHERE n < 100000) INSERT INTO items(id,thread_id,position,seq,status,body) SELECT 'historical-' || n,'thread',n,n,'done','{}' FROM history;").unwrap();
        conn.execute("INSERT INTO items(id,thread_id,position,seq,status,body) VALUES('later','thread',100002,100002,'awaiting_user','{}'),('earlier','thread',100001,100001,'awaiting_user','{}')", []).unwrap();
        conn.execute_batch("ANALYZE").unwrap();
        let mut explain = conn.prepare("EXPLAIN QUERY PLAN SELECT id,thread_id FROM items WHERE status='awaiting_user' ORDER BY position").unwrap();
        let details = explain
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("items_awaiting_position")),
            "the production waiting query must use its tiny ordered index: {details:?}"
        );
        assert!(
            !details.iter().any(|detail| detail.contains("TEMP B-TREE")),
            "waiting identities must not sort historical Items: {details:?}"
        );
        let pending = super::items_awaiting_user(&conn).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|item| item.id.0.as_str())
                .collect::<Vec<_>>(),
            ["earlier", "later"]
        );
    }
}

#[cfg(test)]
mod outline_tests {
    use super::outline_label;

    #[test]
    fn labels_bound_giant_unicode_messages_and_use_first_nonempty_line() {
        assert_eq!(outline_label("\n  \n  hello  \nsecond"), "hello");
        let label = outline_label(&"界".repeat(10_000));
        assert_eq!(label.chars().count(), 161);
        assert!(label.ends_with('…'));
        assert_eq!(outline_label(""), "Image");
    }
}
