//! Queries shared by the read pool and the writer's transactions.

use anyhow::{Context, Result};
use artisan_api::{
    Envelope, Event, Item, ItemId, ItemsPage, ItemsQuery, Project, ProjectId, Run, RunId, Seq,
    Thread, ThreadId,
};
use rusqlite::{Connection, OptionalExtension, Params, params};
use serde::{Serialize, de::DeserializeOwned};

const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 500;

pub(crate) fn to_body<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).context("serialise row body")
}

fn from_body<T: DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).context("parse row body")
}

/// The serde name of a unit enum variant, as stored in status columns.
pub(crate) fn tag<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
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

pub(crate) fn many<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare_cached(sql)?;
    let rows = statement.query_map(params, |row| row.get::<_, String>(0))?;
    rows.map(|body| from_body(&body?)).collect()
}

pub(crate) fn max_seq(conn: &Connection) -> Result<Seq> {
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

pub(crate) fn project(conn: &Connection, id: &ProjectId) -> Result<Option<Project>> {
    one(
        conn,
        "SELECT body FROM projects WHERE id = ?1",
        [id.as_str()],
    )
}

pub(crate) fn thread(conn: &Connection, id: &ThreadId) -> Result<Option<Thread>> {
    one(
        conn,
        "SELECT body FROM threads WHERE id = ?1",
        [id.as_str()],
    )
}

pub(crate) fn item(conn: &Connection, id: &ItemId) -> Result<Option<Item>> {
    one(conn, "SELECT body FROM items WHERE id = ?1", [id.as_str()])
}

pub(crate) fn item_by_native(
    conn: &Connection,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    native_id: &str,
) -> Result<Option<Item>> {
    one(
        conn,
        "SELECT body FROM items
         WHERE thread_id = ?1 AND native_id = ?2 AND (?3 IS NULL OR run_id = ?3)
         ORDER BY position DESC LIMIT 1",
        params![thread_id.as_str(), native_id, run_id.map(RunId::as_str)],
    )
}

/// The names Artisan already gave subagents in a thread.
pub(crate) fn agent_labels(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<String>> {
    let mut statement = conn.prepare_cached(
        "SELECT json_extract(body, '$.kind.label') FROM items
         WHERE thread_id = ?1 AND json_extract(body, '$.kind.type') = 'agent'
           AND json_extract(body, '$.kind.label') IS NOT NULL",
    )?;
    let labels = statement
        .query_map([thread_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(labels)
}

/// Items waiting on the user, across threads.
pub(crate) fn items_awaiting_user(conn: &Connection) -> Result<Vec<Item>> {
    many(
        conn,
        "SELECT body FROM items WHERE status = 'awaiting_user' ORDER BY position",
        [],
    )
}

/// User messages not yet delivered to an engine, oldest first.
pub(crate) fn undelivered_messages(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<Item>> {
    many(
        conn,
        "SELECT body FROM items
         WHERE thread_id = ?1 AND status = 'running'
           AND json_extract(body, '$.kind.type') = 'user_message'
         ORDER BY position",
        [thread_id.as_str()],
    )
}

pub(crate) fn run(conn: &Connection, id: &RunId) -> Result<Option<Run>> {
    one(conn, "SELECT body FROM runs WHERE id = ?1", [id.as_str()])
}

/// Runs are only ever inserted in start order, so rowid orders them.
pub(crate) fn latest_run(conn: &Connection, thread_id: &ThreadId) -> Result<Option<Run>> {
    one(
        conn,
        "SELECT body FROM runs WHERE thread_id = ?1 ORDER BY rowid DESC LIMIT 1",
        [thread_id.as_str()],
    )
}

pub(crate) fn setting(conn: &Connection, key: &str) -> Result<Option<serde_json::Value>> {
    one(conn, "SELECT value FROM kv WHERE key = ?1", [key])
}

pub(crate) fn prompt_version(conn: &Connection) -> Result<u64> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM prompt_versions",
        [],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(version).unwrap_or(0))
}

pub(crate) fn draft(conn: &Connection, scope: &str) -> Result<Option<serde_json::Value>> {
    one(conn, "SELECT body FROM drafts WHERE scope = ?1", [scope])
}

pub(crate) fn items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
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
    let mut items: Vec<Item> = if query.after.is_some() {
        many(
            conn,
            "SELECT body FROM items WHERE thread_id = ?1 AND position > ?2 AND position < ?3
             ORDER BY position ASC LIMIT ?4",
            params![thread_id.as_str(), after, before, fetch],
        )?
    } else {
        let mut items: Vec<Item> = many(
            conn,
            "SELECT body FROM items WHERE thread_id = ?1 AND position < ?2
             ORDER BY position DESC LIMIT ?3",
            params![thread_id.as_str(), before, fetch],
        )?;
        items.reverse();
        items
    };
    let more = items.len() > limit;
    if more {
        if query.after.is_some() {
            items.truncate(limit);
        } else {
            items.remove(0);
        }
    }
    Ok(ItemsPage { items, more })
}

pub(crate) fn changes_since(
    conn: &Connection,
    since: Seq,
    limit: usize,
) -> Result<Option<Vec<Envelope>>> {
    let mut statement = conn.prepare_cached(
        "SELECT seq, 'project', body, NULL FROM projects WHERE seq > ?1
         UNION ALL SELECT seq, 'thread', body, NULL FROM threads WHERE seq > ?1
         UNION ALL SELECT seq, 'item', body, NULL FROM items WHERE seq > ?1
         UNION ALL SELECT seq, 'run', body, NULL FROM runs WHERE seq > ?1
         UNION ALL SELECT seq, 'setting', value, key FROM kv WHERE seq > ?1
         ORDER BY 1 LIMIT ?2",
    )?;
    let rows = statement.query_map(params![since, limit.saturating_add(1)], |row| {
        Ok((
            row.get::<_, Seq>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut envelopes = Vec::new();
    for row in rows {
        let (seq, table, body, key) = row?;
        if envelopes.len() == limit {
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
            event,
        });
    }
    Ok(Some(envelopes))
}
