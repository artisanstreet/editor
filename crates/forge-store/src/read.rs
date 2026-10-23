//! Queries shared by the read pool and the writer's transactions.

use anyhow::{Context, Result};
use artisan_api::{
    Envelope, Event, Item, ItemId, ItemsPage, ItemsQuery, Project, ProjectId, Run, RunId, Seq,
    Thread, ThreadId, WorkSpan,
};
use rusqlite::{Connection, OptionalExtension, Params, params};
use serde::{Serialize, de::DeserializeOwned};

const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 500;

pub fn to_body<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).context("serialise row body")
}

fn from_body<T: DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).context("parse row body")
}

/// The serde name of a unit enum variant, as stored in status columns.
pub fn tag<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .context("stored status must be a unit enum")
        .map(str::to_owned)
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
            SELECT max(seq) FROM kv)",
        [],
        |row| row.get(0),
    )
    .context("read the highest stored seq")
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
        .context("read Items")
}
pub fn item(conn: &Connection, id: &ItemId) -> Result<Option<Item>> {
    conn.prepare_cached("SELECT body FROM items WHERE id=?1")?
        .query_row([id.as_str()], item_row)
        .optional()
        .context("read Item")
}
pub fn item_by_native(
    conn: &Connection,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    native_id: &str,
) -> Result<Option<Item>> {
    conn.prepare_cached("SELECT body FROM items WHERE thread_id=?1 AND native_id=?2 AND (?3 IS NULL OR run_id=?3) ORDER BY position DESC LIMIT 1")?
        .query_row(params![thread_id.as_str(),native_id,run_id.map(RunId::as_str)],item_row).optional().context("read native Item")
}

/// The thread's `Agent` item an engine addresses as `address` (Claude's
/// `agentId`): the latest one, as a resumed agent keeps its address.
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
    .context("read the agent an engine address names")
}

/// The names Artisan already gave subagents in a thread.
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

/// Items waiting on the user, across threads.
pub fn items_awaiting_user(conn: &Connection) -> Result<Vec<artisan_api::PendingItem>> {
    let mut statement = conn.prepare_cached(
        "SELECT id,thread_id FROM items WHERE status='awaiting_user' ORDER BY position",
    )?;
    statement
        .query_map([], |row| {
            Ok(artisan_api::PendingItem {
                id: ItemId(row.get(0)?),
                thread_id: ThreadId(row.get(1)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("read pending interaction identities")
}

pub fn pending_threads(
    conn: &Connection,
    limit: usize,
    excluded: &[ThreadId],
) -> Result<Vec<ThreadId>> {
    // Threads with undelivered messages, and threads asked to continue.
    let mut query = conn.prepare_cached(
        "SELECT thread_id FROM (
           SELECT pending.thread_id, pending.seq FROM items pending
           WHERE pending.status = 'running' AND pending.kind_type = 'user_message'
             AND NOT EXISTS (
               SELECT 1 FROM runs latest WHERE latest.rowid = (
                 SELECT rowid FROM runs WHERE thread_id = pending.thread_id ORDER BY rowid DESC LIMIT 1
               ) AND latest.status IN ('failed','refused')
                 AND pending.seq <= json_extract(latest.body, '$.input_cutoff')
             )
           UNION ALL
           SELECT thread_id, seq FROM continues WHERE due_at IS NULL OR due_at <= ?3
         )
         WHERE thread_id NOT IN (SELECT value FROM json_each(?1))
         GROUP BY thread_id ORDER BY min(seq) LIMIT ?2"
    )?;
    Ok(query
        .query_map(
            params![serde_json::to_string(excluded)?, limit, artisan_api::now()],
            |row| row.get::<_, String>(0),
        )?
        .map(|row| row.map(ThreadId))
        .collect::<rusqlite::Result<_>>()?)
}

pub fn pending_inputs(
    conn: &Connection,
    thread_id: &ThreadId,
    blob_dir: &std::path::Path,
) -> Result<crate::PendingInputs> {
    const BATCH_ROWS: usize = 32;
    const BATCH_BYTES: usize = 4 * 1024 * 1024;
    let tx = conn.unchecked_transaction()?;
    let cutoff = tx.query_row("SELECT coalesce(max(seq),0) FROM items WHERE thread_id=?1 AND status='running' AND kind_type='user_message'", [thread_id.as_str()], |row| row.get(0))?;
    let mut query = tx.prepare_cached("SELECT body FROM items WHERE thread_id=?1 AND status='running' AND kind_type='user_message' ORDER BY position LIMIT 32")?;
    let mut rows = query.query([thread_id.as_str()])?;
    let mut items = Vec::with_capacity(BATCH_ROWS);
    let mut bytes = 0usize;
    let mut image_bytes = 0u64;
    while let Some(row) = rows.next()? {
        let body = row.get_ref(0)?.as_str()?;
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
            // A bad historical reference remains in the first batch. The
            // coordinator validates it after Run creation and settles Failed.
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
    Ok(crate::PendingInputs { items, cutoff })
}

pub fn run(conn: &Connection, id: &RunId) -> Result<Option<Run>> {
    one(conn, "SELECT body FROM runs WHERE id = ?1", [id.as_str()])
}

/// Runs are only ever inserted in start order, so rowid orders them.
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

/// `Some(None)` is the root transcript; `Some(Some(id))` is one agent's own work.
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
    // Without `after` the page is the newest items before `before` (the tail);
    // with it the page reads forward from `after`.
    let order = if query.after.is_some() { "ASC" } else { "DESC" };
    let sql = match parent {
        // An agent's own work, plus the messages the root sent it.
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
        // Every persisted Item is compact after source normalization. No
        // oversized-item exception or whole-source allocation is needed here.
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
        [serde_json::to_string(&ids)?],
    )?;
    snapshot.commit()?;
    Ok(ItemsPage {
        items,
        runs,
        more,
        work: Vec::new(),
    })
}

/// Items per page: `limit`, or the default, within bounds.
fn page_limit(query: &ItemsQuery) -> usize {
    query
        .limit
        .map_or(DEFAULT_PAGE, |limit| {
            usize::try_from(limit).unwrap_or(MAX_PAGE)
        })
        .clamp(1, MAX_PAGE)
}

/// A page's position bounds, exclusive: `(after, before)`.
fn page_bounds(query: &ItemsQuery) -> (i64, i64) {
    let after = query
        .after
        .map_or(0, |after| i64::try_from(after).unwrap_or(i64::MAX));
    let before = query
        .before
        .map_or(i64::MAX, |before| i64::try_from(before).unwrap_or(i64::MAX));
    (after, before)
}

/// Reads up to `limit` item bodies from `rows`, the first always and the
/// rest within the page's byte budget. Returns them and whether more exist.
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

/// The runs `items` and `work` belong to.
fn page_runs(conn: &Connection, items: &[Item], work: &[WorkSpan]) -> Result<Vec<Run>> {
    let ids: std::collections::BTreeSet<_> = items
        .iter()
        .filter_map(|item| item.run_id.as_ref())
        .chain(work.iter().filter_map(|span| span.run_id.as_ref()))
        .collect();
    many(
        conn,
        "SELECT body FROM runs WHERE id IN (SELECT value FROM json_each(?1))",
        [serde_json::to_string(&ids)?],
    )
}

/// The root work strictly between positions `after` and `before`, as one
/// span per run's unbroken stretch: a row or another run's item ends one.
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

/// A page of the root transcript as the reader sees it: up to `limit`
/// items that stand on their own (the newest before `before`, or the
/// oldest after `after`) and the work between them as spans, so a turn of
/// any length is one row's worth. The page owns the work from its far row
/// to its near bound; the next page owns the work beyond the far row. A
/// live run's newest work ships as items: it is always open, and its
/// phase reads from it.
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
    // With more rows beyond it, the page's work stops at its far row.
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
    for run in runs.iter().filter(|run| run.status.is_live()) {
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
                // The rest of the span ends just before what shipped.
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

/// The root work strictly between `after` and `before`, newest first: an
/// open work section loading its span from the end the reader meets.
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
    Ok(Some(envelopes))
}

/// Compact current controls, independent of any transcript page. Every
/// query is indexed by thread and kind, so it is computed per request.
pub fn inspector(conn: &Connection, thread: &ThreadId) -> Result<artisan_api::InspectorSummary> {
    use artisan_api::{InspectorSummary, InspectorTerminal, InspectorWorkflow};
    #[derive(serde::Deserialize)]
    struct WorkflowRow {
        id: artisan_api::ItemId,
        status: artisan_api::Status,
        input: Option<artisan_api::TextContent>,
    }
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
    let revision = conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM (SELECT seq FROM items WHERE thread_id=?1 UNION ALL SELECT seq FROM runs WHERE thread_id=?1)",
        [thread.as_str()],
        |row| row.get(0),
    )?;
    // Work counts as background when the engine started it there, or when it
    // runs on after its run ended (a Codex command left running).
    let terminals: Vec<InspectorTerminal> = many(conn,
        "SELECT json_object('id',id,'command',json_extract(body,'$.kind.tool.command'),
            'cwd',json_extract(body,'$.kind.tool.cwd'),'status',status,
            'background',json(CASE WHEN COALESCE(json_extract(body,'$.kind.tool.background'),0)
                OR (SELECT r.status FROM runs r WHERE r.id=items.run_id)!='running'
                THEN 'true' ELSE 'false' END),
            'watch',json(CASE WHEN COALESCE(json_extract(body,'$.kind.tool.watch'),0)
                THEN 'true' ELSE 'false' END)) FROM items
         WHERE thread_id=?1 AND status IN ('running','awaiting_user') AND json_extract(body,'$.kind.tool.kind')='shell' ORDER BY position DESC",
         [thread.as_str()])?;
    let agents = live_agents(conn, thread)?;
    let workflows: Vec<InspectorWorkflow> = many::<WorkflowRow>(conn,
        "SELECT json_object('id',id,'status',status,'input',json(body->'$.kind.tool.input')) FROM items
         WHERE thread_id=?1 AND status IN ('running','awaiting_user')
            AND json_extract(body,'$.kind.tool.kind')='other'
            AND json_extract(body,'$.kind.tool.name')='Workflow' ORDER BY position DESC",
        [thread.as_str()])?
    .into_iter()
    .map(|row| InspectorWorkflow {
        id: row.id,
        name: match row.input {
            Some(artisan_api::TextContent::Inline(input)) => workflow_name(&input),
            _ => None,
        },
        status: row.status,
    })
    .collect();
    let plan = conn
        .query_row(
            "SELECT json_extract(body,'$.kind.steps') FROM items
        WHERE thread_id=?1 AND kind_type='plan'
            AND parent_id IS NULL ORDER BY position DESC LIMIT 1",
            [thread.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|body| from_body(&body))
        .transpose()?
        .unwrap_or_default();
    let goal = current_goal(conn, thread)?;
    let questions = many(conn,
        "SELECT json_object('id',id,'questions',json(json_extract(body,'$.kind.questions'))) FROM items
            WHERE thread_id=?1 AND status='awaiting_user' AND kind_type='question'
            AND json_extract(body,'$.kind.answers') IS NULL ORDER BY position ASC",[thread.as_str()])?;
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

/// The thread's agents still at work, for the inspector.
fn live_agents(conn: &Connection, thread: &ThreadId) -> Result<Vec<artisan_api::InspectorAgent>> {
    // Only agents still at work: one that settled leaves the list at once,
    // not when the next turn starts.
    let mut agents: Vec<artisan_api::InspectorAgent> = many(
        conn,
        "SELECT json_object('id',id,'label',COALESCE(json_extract(body,'$.kind.label'),''),
            'description',json_extract(body,'$.kind.description'),'status',status,
            'engine',COALESCE(json_extract(body,'$.kind.engine'),
                (SELECT json_extract(t.body,'$.config.engine') FROM threads t WHERE t.id=?1)),
            'model',json_extract(body,'$.kind.model'),'thread',json_extract(body,'$.kind.thread'),
            'config',json(COALESCE(
                (SELECT json_extract(t.body,'$.config') FROM threads t WHERE t.id=json_extract(items.body,'$.kind.thread')),
                (SELECT json_extract(t.body,'$.config') FROM threads t WHERE t.id=?1))),
            'background',json(CASE WHEN COALESCE(json_extract(body,'$.kind.background'),0)
                OR (SELECT r.status FROM runs r WHERE r.id=items.run_id)!='running'
                THEN 'true' ELSE 'false' END)) FROM items
         WHERE thread_id=?1 AND kind_type='agent'
            AND status IN ('running','awaiting_user') ORDER BY position DESC",
        [thread.as_str()],
    )?;
    for agent in &mut agents {
        if agent.label.is_empty() {
            agent.label = artisan_api::fallback_agent_name(agent.id.as_str()).into();
        }
        // An engine's own subagent runs on the thread's settings but its own model.
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

/// The thread's current goal, for the inspector's strip.
fn current_goal(conn: &Connection, thread: &ThreadId) -> Result<Option<artisan_api::ThreadGoal>> {
    conn.query_row(
        "SELECT json_extract(body,'$.kind.kind.state'),json_quote(json_extract(body,'$.kind.text')) FROM items
            WHERE thread_id=?1 AND parent_id IS NULL
            AND kind_type='notice' AND json_extract(body,'$.kind.kind.type')='goal'
            ORDER BY seq DESC LIMIT 1",
        [thread.as_str()],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))
    ).optional()?.and_then(|(state,objective)| {
        let state = match state.as_str() {
            "active" => artisan_api::ThreadGoalState::Active,
            "paused" => artisan_api::ThreadGoalState::Paused,
            "blocked" => artisan_api::ThreadGoalState::Blocked,
            "usageLimited" => artisan_api::ThreadGoalState::UsageLimited,
            "budgetLimited" => artisan_api::ThreadGoalState::BudgetLimited,
            // Complete and cleared goals leave the strip.
            _ => return None,
        };
        Some(from_body::<artisan_api::TextContent>(&objective).and_then(|objective| {
            let prefix = match &objective {
                artisan_api::TextContent::Inline(text) => std::borrow::Cow::Borrowed(text.as_str()),
                artisan_api::TextContent::Stored(source) => std::borrow::Cow::Owned(crate::content::range(conn, &source.id,
                    &artisan_api::ContentQuery { offset: 0, limit: Some(1024) })?
                    .context("current goal source unavailable")?.text),
            };
            let mut summary: String = prefix.chars().take(160).collect();
            if objective.byte_len() > summary.len() as u64 { summary.push('…'); }
            Ok(artisan_api::ThreadGoal { objective, summary, state })
        }))
    }).transpose()
}

/// A Claude workflow's name: a saved one's, else the `name` in its script's
/// `meta`.
fn workflow_name(input: &str) -> Option<String> {
    let input: serde_json::Value = serde_json::from_str(input).ok()?;
    if let Some(name) = input["name"].as_str() {
        return Some(name.to_owned());
    }
    let script = input["script"].as_str()?;
    let meta = &script[script.find("meta")?..];
    let rest = meta[meta.find("name")? + 4..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '\'' | '"' | '`'))?;
    let name = &rest[1..];
    Some(name[..name.find(quote)?].to_owned())
}

/// The user's own messages in `thread`, in order, each labelled by its first
/// line. A steer sits inside its run's work, and an agent's result Forge
/// delivered reads as that agent's report, so neither is one.
fn outline(conn: &Connection, thread: &ThreadId) -> Result<Vec<artisan_api::OutlineEntry>> {
    let mut statement = conn.prepare_cached(
        "SELECT id,position,json_quote(json_extract(body,'$.kind.text')) FROM items
         WHERE thread_id=?1 AND kind_type='user_message' AND json_extract(body,'$.parent_id') IS NULL
            AND NOT (run_id IS NOT NULL AND json_extract(body,'$.kind.steer')=1)
            AND json_extract(body,'$.kind.from_agent') IS NULL
         ORDER BY position ASC",
    )?;
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
        let prefix = match &text {
            artisan_api::TextContent::Inline(text) => std::borrow::Cow::Borrowed(text.as_str()),
            artisan_api::TextContent::Stored(source) => std::borrow::Cow::Owned(
                crate::content::range(
                    conn,
                    &source.id,
                    &artisan_api::ContentQuery {
                        offset: 0,
                        limit: Some(1024),
                    },
                )?
                .map(|range| range.text)
                .unwrap_or_default(),
            ),
        };
        Ok(artisan_api::OutlineEntry {
            id: ItemId(id),
            position,
            label: outline_label(&prefix),
        })
    })
    .collect()
}

/// A message's first non-empty line, at most 160 characters; "Image" for one
/// that is only attachments.
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

pub fn threads_page(
    conn: &Connection,
    query: &artisan_api::ThreadsQuery,
) -> Result<artisan_api::ThreadsPage> {
    let limit = query.limit.unwrap_or(100).clamp(1, 200) as usize;
    anyhow::ensure!(
        query.before_activity.is_some() == query.before_id.is_some()
            && query.after_activity.is_some() == query.after_id.is_some(),
        "incomplete thread cursor"
    );
    anyhow::ensure!(
        query.before_id.is_none() || query.after_id.is_none(),
        "a thread page is either before or after its cursor"
    );
    // A newer page walks up from its cursor, then reads newest first like
    // every other page.
    let newer = query.after_id.is_some();
    let (cursor, id) = if newer {
        (
            query.after_activity.unwrap_or(0).min(i64::MAX as u64),
            query.after_id.as_ref().map_or("", ThreadId::as_str),
        )
    } else {
        (
            query
                .before_activity
                .unwrap_or(i64::MAX as u64)
                .min(i64::MAX as u64),
            query.before_id.as_ref().map_or("", ThreadId::as_str),
        )
    };
    let search = query.query.as_deref().unwrap_or("").trim();
    anyhow::ensure!(search.len() <= 256, "thread search exceeds 256 bytes");
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
    let ids = serde_json::to_string(&threads.iter().map(|thread| &thread.id).collect::<Vec<_>>())?;
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
