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
pub(crate) fn tag<T: Serialize>(value: &T) -> Result<String> {
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

fn item_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}
pub(crate) fn items(conn: &Connection, sql: &str, params: impl Params) -> Result<Vec<Item>> {
    conn.prepare_cached(sql)?
        .query_map(params, item_row)?
        .collect::<rusqlite::Result<_>>()
        .context("read Items")
}
pub(crate) fn item(conn: &Connection, id: &ItemId) -> Result<Option<Item>> {
    conn.prepare_cached("SELECT body FROM items WHERE id=?1")?
        .query_row([id.as_str()], item_row)
        .optional()
        .context("read Item")
}
pub(crate) fn item_by_native(
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
pub(crate) fn agent_by_address(
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
pub(crate) fn agent_labels(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<String>> {
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
pub(crate) fn items_awaiting_user(conn: &Connection) -> Result<Vec<artisan_api::PendingItem>> {
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

pub(crate) fn pending_threads(
    conn: &Connection,
    limit: usize,
    excluded: &[ThreadId],
) -> Result<Vec<ThreadId>> {
    let mut query = conn.prepare_cached(
        "SELECT pending.thread_id FROM items pending
         WHERE pending.status = 'running' AND pending.kind_type = 'user_message'
           AND pending.thread_id NOT IN (SELECT value FROM json_each(?1))
           AND NOT EXISTS (
             SELECT 1 FROM runs latest WHERE latest.rowid = (
               SELECT rowid FROM runs WHERE thread_id = pending.thread_id ORDER BY rowid DESC LIMIT 1
             ) AND latest.status IN ('failed','refused')
               AND pending.seq <= json_extract(latest.body, '$.input_cutoff')
           )
         GROUP BY pending.thread_id ORDER BY min(pending.seq) LIMIT ?2"
    )?;
    Ok(query
        .query_map(params![serde_json::to_string(excluded)?, limit], |row| {
            row.get::<_, String>(0)
        })?
        .map(|row| row.map(ThreadId))
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn pending_inputs(
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
                .map(|(_, bytes)| bytes)
                .unwrap_or(128 * 1024 * 1024 + 1)
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

pub(crate) fn draft(conn: &Connection, scope: &str) -> Result<artisan_api::DraftSnapshot> {
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

pub(crate) fn items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
) -> Result<ItemsPage> {
    scoped_items_page(conn, thread_id, query, None)
}

/// `Some(None)` is the root transcript; `Some(Some(id))` is one agent's own work.
pub(crate) fn scoped_items_page(
    conn: &Connection,
    thread_id: &ThreadId,
    query: &ItemsQuery,
    parent: Option<Option<&ItemId>>,
) -> Result<ItemsPage> {
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
    const PAGE_BYTES: usize = 8 * 1024 * 1024;
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
    Ok(ItemsPage { items, runs, more })
}

pub(crate) fn changes_since(
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
pub(crate) fn inspector(
    conn: &Connection,
    thread: &ThreadId,
) -> Result<artisan_api::InspectorSummary> {
    use artisan_api::{InspectorAgent, InspectorSummary, InspectorTerminal};
    let snapshot =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
    let revision = conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM (SELECT seq FROM items WHERE thread_id=?1 UNION ALL SELECT seq FROM runs WHERE thread_id=?1)",
        [thread.as_str()],
        |row| row.get(0),
    )?;
    let latest: Option<String> = conn
        .query_row(
            "SELECT id FROM runs WHERE thread_id=?1 ORDER BY seq DESC LIMIT 1",
            [thread.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    let terminals: Vec<InspectorTerminal> = many(conn,
        "SELECT json_object('id',id,'command',json_extract(body,'$.kind.tool.command'),
            'cwd',json_extract(body,'$.kind.tool.cwd'),'status',status) FROM items
         WHERE thread_id=?1 AND status IN ('running','awaiting_user') AND json_extract(body,'$.kind.tool.kind')='shell' ORDER BY position DESC",
         [thread.as_str()])?;
    let mut agents: Vec<InspectorAgent> = many(
        conn,
        "SELECT json_object('id',id,'label',COALESCE(json_extract(body,'$.kind.label'),''),
            'description',json_extract(body,'$.kind.description'),'status',status) FROM items
         WHERE thread_id=?1 AND kind_type='agent'
            AND (status IN ('running','awaiting_user') OR run_id=?2) ORDER BY position DESC",
        params![thread.as_str(), latest],
    )?;
    for agent in &mut agents {
        if agent.label.is_empty() {
            agent.label = artisan_api::fallback_agent_name(agent.id.as_str()).into();
        }
    }
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
    let goal = conn.query_row(
        "SELECT json_extract(body,'$.kind.kind.state'),json_quote(json_extract(body,'$.kind.text')) FROM items
            WHERE thread_id=?1 AND parent_id IS NULL
            AND kind_type='notice' AND json_extract(body,'$.kind.kind.type')='goal'
            ORDER BY position DESC LIMIT 1",
        [thread.as_str()],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))
    ).optional()?.and_then(|(state,objective)| {
        let state = match state.as_str() {
            "active" => artisan_api::ThreadGoalState::Active,
            "paused" => artisan_api::ThreadGoalState::Paused,
            _ => return None,
        };
        Some(from_body::<artisan_api::TextContent>(&objective).and_then(|objective| {
            let prefix = match &objective {
                artisan_api::TextContent::Inline(text) => std::borrow::Cow::Borrowed(text.as_str()),
                artisan_api::TextContent::Stored(source) => std::borrow::Cow::Owned(crate::content::range(conn, &source.id,
                    artisan_api::ContentQuery { offset: 0, limit: Some(1024) })?
                    .context("current goal source unavailable")?.text),
            };
            let mut summary: String = prefix.chars().take(160).collect();
            if objective.byte_len() > summary.len() as u64 { summary.push('…'); }
            Ok(artisan_api::ThreadGoal { objective, summary, state })
        }))
    }).transpose()?;
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
    snapshot.commit()?;
    Ok(InspectorSummary {
        goal,
        questions,
        revision,
        terminals,
        agents,
        plan,
        edits,
    })
}

pub(crate) fn threads_page(
    conn: &Connection,
    query: &artisan_api::ThreadsQuery,
) -> Result<artisan_api::ThreadsPage> {
    let limit = query.limit.unwrap_or(100).clamp(1, 200) as usize;
    let before = query
        .before_activity
        .unwrap_or(i64::MAX as u64)
        .min(i64::MAX as u64);
    anyhow::ensure!(
        query.before_activity.is_some() == query.before_id.is_some(),
        "incomplete thread cursor"
    );
    let id = query.before_id.as_ref().map_or("", ThreadId::as_str);
    let search = query.query.as_deref().unwrap_or("").trim();
    anyhow::ensure!(search.len() <= 256, "thread search exceeds 256 bytes");
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let base =
        "SELECT t.body FROM threads t JOIN projects p ON p.id=json_extract(t.body,'$.project_id')
         WHERE COALESCE(json_extract(t.body,'$.archived'),0)=?1
          AND COALESCE(json_extract(p.body,'$.removed'),0)=0
          AND (json_extract(t.body,'$.activity_seq'),t.id)<(?2,?3)";
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
        "{base}{search_clause} ORDER BY json_extract(t.body,'$.activity_seq') DESC,t.id DESC LIMIT ?5"
    );
    let mut threads: Vec<Thread> = many(
        conn,
        &sql,
        params![query.archived, before, id, search, limit + 1],
    )?;
    let more = threads.len() > limit;
    threads.truncate(limit);
    let next = if more {
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
