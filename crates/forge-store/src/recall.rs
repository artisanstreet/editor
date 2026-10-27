//! Recall: what an agent finds across every thread.
//!
//! Each item's readable text (messages, plans, commands, paths, errors) is
//! kept in `item_text` beside the item, under the item's rowid, and indexed
//! for ranked search in `item_search`. Tool output has its own column: it is
//! scanned only when a search asks for it, so logs don't drown the
//! conversation. Both are written in the transaction that saves the item.

use crate::Store;
use anyhow::{Context, Result};
use artisan_api::{
    Collection, ContentQuery, EngineId, FileOp, Item, ItemId, ItemKind, ProjectId, Status,
    TextContent, ThreadId, Timestamp, ToolKind,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// The most of one text field an item keeps searchable.
const FIELD_BYTES: usize = 64 * 1024;
/// Rows one grep reads before it hands back a cursor.
const GREP_SCAN_ROWS: usize = 20_000;

/// What one item contributes to recall.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Searchable {
    pub(crate) text: String,
    pub(crate) output: String,
    /// One `<op>\t<path>` line per path the item read or changed.
    pub(crate) paths: String,
}

impl Searchable {
    const fn is_empty(&self) -> bool {
        self.text.is_empty() && self.output.is_empty() && self.paths.is_empty()
    }
}

fn push(field: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() || field.len() >= FIELD_BYTES {
        return;
    }
    if !field.is_empty() {
        field.push('\n');
    }
    let mut end = text.len().min(FIELD_BYTES - field.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    field.push_str(&text[..end]);
}

/// The first [`FIELD_BYTES`] of a text, inline or stored.
fn text_of(conn: &Connection, text: &TextContent) -> Result<String> {
    match text {
        TextContent::Inline(text) => Ok(text.clone()),
        TextContent::Stored(source) => Ok(crate::content::range(
            conn,
            &source.id,
            &ContentQuery {
                offset: 0,
                limit: Some(u32::try_from(FIELD_BYTES)?),
            },
        )?
        .map(|range| range.text)
        .unwrap_or_default()),
    }
}

fn entries<T: serde::de::DeserializeOwned + Clone>(
    conn: &Connection,
    collection: &Collection<T>,
) -> Result<Vec<T>> {
    match collection {
        Collection::Inline(entries) => Ok(entries.clone()),
        Collection::Stored(_) => crate::collection::resolve(conn, collection),
    }
}

pub fn searchable(conn: &Connection, item: &Item) -> Result<Searchable> {
    let mut found = Searchable::default();
    // Recall's own calls would only find themselves.
    if artisan_call(item) {
        return Ok(found);
    }
    let text = &mut found.text;
    match &item.kind {
        ItemKind::UserMessage { text: body, .. }
        | ItemKind::AssistantMessage { text: body, .. } => {
            push(text, &text_of(conn, body)?);
        }
        ItemKind::Tool {
            tool,
            title,
            output,
            exit_code,
            ..
        } => {
            push(text, title.as_deref().unwrap_or_default());
            push_tool(conn, tool, text, &mut found.paths)?;
            if let Some(output) = output.as_ref().filter(|_| item.status.is_terminal()) {
                let output = text_of(conn, &output.text)?;
                // A failed command's output is what someone searches for.
                if exit_code.is_some_and(|code| code != 0) || item.status == Status::Failed {
                    push(text, &output);
                }
                push(&mut found.output, &output);
            }
        }
        ItemKind::Agent {
            label,
            description,
            prompt,
            result,
            ..
        } => {
            push(text, label.as_deref().unwrap_or_default());
            push(text, &text_of(conn, description)?);
            if let Some(prompt) = prompt {
                push(text, &text_of(conn, prompt)?);
            }
            if let Some(result) = result {
                push(text, &text_of(conn, result)?);
            }
        }
        ItemKind::Question {
            questions,
            answer_text,
            ..
        } => {
            for question in entries(conn, questions)? {
                push(text, &text_of(conn, &question.text)?);
            }
            if let Some(answer) = answer_text {
                push(text, &text_of(conn, answer)?);
            }
        }
        ItemKind::Plan {
            steps, proposal, ..
        } => {
            if let Some(proposal) = proposal {
                push(text, &text_of(conn, proposal)?);
            }
            for step in entries(conn, steps)? {
                push(text, &text_of(conn, &step.text)?);
            }
        }
        ItemKind::Image { caption, .. } => {
            if let Some(caption) = caption {
                push(text, &text_of(conn, caption)?);
            }
        }
        ItemKind::Notice { text: body, .. } => push(text, &text_of(conn, body)?),
        ItemKind::Error { message, .. } => push(text, &text_of(conn, message)?),
        ItemKind::Compaction { summary, .. } => {
            if let Some(summary) = summary {
                push(text, &text_of(conn, summary)?);
            }
        }
    }
    Ok(found)
}

/// What a tool call adds to the searchable text and the paths it touched.
fn push_tool(
    conn: &Connection,
    tool: &ToolKind,
    text: &mut String,
    paths: &mut String,
) -> Result<()> {
    match tool {
        ToolKind::AgentMessage { text: body, .. } => push(text, &text_of(conn, body)?),
        ToolKind::Agents { .. } => {}
        ToolKind::Shell { command, .. } => push(text, &text_of(conn, command)?),
        ToolKind::Read { path, .. } | ToolKind::ViewImage { path } => {
            push(paths, &format!("read\t{path}"));
        }
        ToolKind::Edit { changes } => {
            for change in entries(conn, changes)? {
                let op = match change.op {
                    FileOp::Add => "create",
                    FileOp::Delete => "delete",
                    FileOp::Modify | FileOp::Move { .. } => "edit",
                };
                push(paths, &format!("{op}\t{}", change.path));
            }
        }
        ToolKind::Search { pattern, path, .. } => {
            push(text, pattern);
            if let Some(path) = path {
                push(paths, &format!("search\t{path}"));
            }
        }
        ToolKind::WebFetch { url } => push(text, url),
        ToolKind::WebSearch { query } => push(text, query),
        ToolKind::Mcp {
            server,
            tool,
            input,
        } => {
            push(text, &format!("{server} {tool}"));
            if let Some(input) = input {
                push(text, &text_of(conn, input)?);
            }
        }
        ToolKind::Other { name, input } => {
            push(text, name);
            if let Some(input) = input {
                push(text, &text_of(conn, input)?);
            }
        }
    }
    Ok(())
}

/// Brings the item's recall rows in line with its saved body. Unchanged text
/// writes nothing.
/// A call to Artisan's own MCP server.
fn artisan_call(item: &Item) -> bool {
    matches!(&item.kind, ItemKind::Tool { tool: ToolKind::Mcp { server, .. }, .. } if server == "artisan")
}

/// Takes the Artisan calls the first index kept out of it.
pub fn forget_artisan_calls(conn: &Connection) -> Result<()> {
    let mut rows = conn.prepare(
        "SELECT body FROM items WHERE kind_type='tool'
            AND json_extract(body,'$.kind.tool.kind')='mcp'
            AND json_extract(body,'$.kind.tool.server')='artisan'",
    )?;
    let bodies = rows.query_map([], |row| row.get::<_, String>(0))?;
    for body in bodies {
        if let Ok(item) = serde_json::from_str::<Item>(&body?) {
            index(conn, &item).with_context(|| format!("unindex Artisan call {}", item.id))?;
        }
    }
    Ok(())
}

pub fn index(conn: &Connection, item: &Item) -> Result<()> {
    let rowid: i64 = conn
        .prepare_cached("SELECT rowid FROM items WHERE id=?1")?
        .query_row([item.id.as_str()], |row| row.get(0))
        .with_context(|| format!("find item {} to index", item.id))?;
    let next = searchable(conn, item)?;
    let previous = conn
        .prepare_cached("SELECT text,output,paths FROM item_text WHERE rowid=?1")?
        .query_row([rowid], |row| {
            Ok(Searchable {
                text: row.get(0)?,
                output: row.get(1)?,
                paths: row.get(2)?,
            })
        })
        .optional()?;
    if previous.as_ref() == Some(&next) {
        return Ok(());
    }
    if let Some(previous) = &previous {
        conn.prepare_cached(
            "INSERT INTO item_search(item_search,rowid,text,paths) VALUES('delete',?1,?2,?3)",
        )?
        .execute(params![rowid, previous.text, previous.paths])?;
    }
    if next.is_empty() {
        conn.prepare_cached("DELETE FROM item_text WHERE rowid=?1")?
            .execute([rowid])?;
        return Ok(());
    }
    conn.prepare_cached(
        "INSERT OR REPLACE INTO item_text(rowid,thread_id,item_id,kind,created_at,text,output,paths)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
    )?
    .execute(params![
        rowid,
        item.thread_id.as_str(),
        item.id.as_str(),
        kind_name(&item.kind),
        item.created_at,
        next.text,
        next.output,
        next.paths,
    ])?;
    conn.prepare_cached("INSERT INTO item_search(rowid,text,paths) VALUES(?1,?2,?3)")?
        .execute(params![rowid, next.text, next.paths])?;
    Ok(())
}

/// The kind as an agent filters by it.
const fn kind_name(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::UserMessage { .. } => "user",
        ItemKind::AssistantMessage { .. } => "assistant",
        ItemKind::Tool { .. } => "tool",
        ItemKind::Agent { .. } => "agent",
        ItemKind::Question { .. } => "question",
        ItemKind::Plan { .. } => "plan",
        ItemKind::Image { .. } => "image",
        ItemKind::Compaction { .. } => "compaction",
        ItemKind::Notice { .. } => "notice",
        ItemKind::Error { .. } => "error",
    }
}

/// Indexes every item already stored (the migration that adds recall).
pub fn backfill(conn: &Connection) -> Result<()> {
    let mut rows = conn.prepare("SELECT body FROM items ORDER BY rowid")?;
    let bodies = rows.query_map([], |row| row.get::<_, String>(0))?;
    let mut skipped = 0_u64;
    for body in bodies {
        let body = body?;
        let Ok(item) = serde_json::from_str::<Item>(&body) else {
            skipped += 1;
            continue;
        };
        index(conn, &item).with_context(|| format!("index item {}", item.id))?;
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            "items left out of recall: their bodies didn't read"
        );
    }
    Ok(())
}

/// Which threads a recall query looks in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallScope {
    pub thread: Option<ThreadId>,
    pub project: Option<ProjectId>,
    pub engine: Option<EngineId>,
    /// Item kinds ("user", "assistant", "tool", "agent", "plan", ...).
    #[serde(default)]
    pub kinds: Vec<String>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    /// Leave archived threads out.
    #[serde(default)]
    pub skip_archived: bool,
}

impl RecallScope {
    /// The scope as SQL over `item_text t` joined to `threads h`, with its
    /// parameters starting at `?first`.
    fn sql(&self, first: usize) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut clauses = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        let mut add = |clause: &str, value: Value| {
            clauses.push(clause.replace('?', &format!("?{}", first + values.len())));
            values.push(value);
        };
        if let Some(thread) = &self.thread {
            add("t.thread_id=?", Value::Text(thread.as_str().to_owned()));
        }
        if let Some(project) = &self.project {
            add(
                "json_extract(h.body,'$.project_id')=?",
                Value::Text(project.as_str().to_owned()),
            );
        }
        if let Some(engine) = self.engine {
            add(
                "json_extract(h.body,'$.config.engine')=?",
                Value::Text(engine.as_str().to_owned()),
            );
        }
        if let Some(since) = self.since {
            add("t.created_at>=?", Value::Integer(since));
        }
        if let Some(until) = self.until {
            add("t.created_at<?", Value::Integer(until));
        }
        if !self.kinds.is_empty() {
            let list = serde_json::to_string(&self.kinds).unwrap_or_default();
            add(
                "t.kind IN (SELECT value FROM json_each(?))",
                Value::Text(list),
            );
        }
        if self.skip_archived {
            clauses.push("COALESCE(json_extract(h.body,'$.archived'),0)=0".to_owned());
        }
        let sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" AND {}", clauses.join(" AND "))
        };
        (sql, values)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub thread_id: ThreadId,
    pub thread_title: Option<String>,
    pub item_id: ItemId,
    pub kind: String,
    pub created_at: Timestamp,
    /// The matching text around the match, `[`marked`]`.
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Pass back as `offset` for the next page; `None` at the end.
    pub next: Option<u32>,
}

/// An FTS query matching every word of `query` anywhere, or `None` when no
/// word is long enough for the trigram index.
fn fts_query(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split_whitespace()
        .filter(|word| word.chars().count() >= 3)
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" AND "))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepMatch {
    pub thread_id: ThreadId,
    pub thread_title: Option<String>,
    pub item_id: ItemId,
    pub kind: String,
    pub created_at: Timestamp,
    /// "text" or "output".
    pub field: String,
    /// 1-based, within the field.
    pub line: u32,
    pub before: Vec<String>,
    pub text: String,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepPage {
    pub matches: Vec<GrepMatch>,
    /// Pass back as `cursor` to keep going (older items); `None` once every
    /// item in scope was read.
    pub next: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepQuery {
    pub pattern: String,
    #[serde(default)]
    pub ignore_case: bool,
    /// Also read tool output.
    #[serde(default)]
    pub output: bool,
    #[serde(default)]
    pub context: u32,
    pub max_matches: Option<u32>,
    /// Items older than this rowid; from a previous page's `next`.
    pub cursor: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Touch {
    pub thread_id: ThreadId,
    pub thread_title: Option<String>,
    pub item_id: ItemId,
    pub op: String,
    pub path: String,
    pub created_at: Timestamp,
}

fn grep_rows(conn: &Connection, scope: &RecallScope, query: &GrepQuery) -> Result<GrepPage> {
    let regex = regex::RegexBuilder::new(&query.pattern)
        .case_insensitive(query.ignore_case)
        .size_limit(1 << 20)
        .build()
        .map_err(|error| crate::Refused(format!("That pattern doesn't compile: {error}")))?;
    let max = query.max_matches.unwrap_or(50).clamp(1, 500) as usize;
    let context = query.context.min(10) as usize;
    let (filter, mut values) = scope.sql(2);
    values.insert(
        0,
        rusqlite::types::Value::Integer(query.cursor.unwrap_or(i64::MAX)),
    );
    let output = if query.output { "t.output" } else { "''" };
    let sql = format!(
        "SELECT t.rowid,t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.text,{output},t.created_at
         FROM item_text t JOIN threads h ON h.id=t.thread_id
         WHERE t.rowid<?1{filter} ORDER BY t.rowid DESC LIMIT {}",
        GREP_SCAN_ROWS + 1
    );
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(values))?;
    let mut matches = Vec::new();
    let mut scanned = 0;
    let mut last = None;
    while let Some(row) = rows.next()? {
        scanned += 1;
        if scanned > GREP_SCAN_ROWS {
            return Ok(GrepPage {
                matches,
                next: last,
            });
        }
        let rowid: i64 = row.get(0)?;
        last = Some(rowid);
        let thread_id = ThreadId::from(row.get::<_, String>(1)?);
        let thread_title: Option<String> = row.get(2)?;
        let item_id = ItemId::from(row.get::<_, String>(3)?);
        let kind: String = row.get(4)?;
        let created_at: Timestamp = row.get(7)?;
        for (field, body) in [("text", row.get::<_, String>(5)?), ("output", row.get(6)?)] {
            if !regex.is_match(&body) {
                continue;
            }
            let lines: Vec<&str> = body.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                let clip = |line: &&str| -> String { line.chars().take(400).collect() };
                matches.push(GrepMatch {
                    thread_id: thread_id.clone(),
                    thread_title: thread_title.clone(),
                    item_id: item_id.clone(),
                    kind: kind.clone(),
                    created_at,
                    field: field.to_owned(),
                    line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                    before: lines[index.saturating_sub(context)..index]
                        .iter()
                        .map(clip)
                        .collect(),
                    text: clip(line),
                    after: lines[index + 1..(index + 1 + context).min(lines.len())]
                        .iter()
                        .map(clip)
                        .collect(),
                });
                if matches.len() >= max {
                    return Ok(GrepPage {
                        matches,
                        next: Some(rowid),
                    });
                }
            }
        }
    }
    Ok(GrepPage {
        matches,
        next: None,
    })
}

fn search_rows(
    conn: &Connection,
    scope: &RecallScope,
    query: &str,
    output: bool,
    limit: u32,
    offset: u32,
) -> Result<SearchPage> {
    let limit = limit.clamp(1, 100);
    let (filter, scope_values) = scope.sql(5);
    // The whole query as typed ranks first: an exact phrase beats its words
    // scattered through a long tool call.
    let mut values = vec![
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Integer(i64::from(limit) + 1),
        rusqlite::types::Value::Integer(i64::from(offset)),
        rusqlite::types::Value::Text(query.trim().to_owned()),
    ];
    let sql = if let Some(fts) = fts_query(query) {
        values[0] = rusqlite::types::Value::Text(fts);
        format!(
                "SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
                    snippet(item_search,0,'**','**','…',24)
                 FROM item_search s JOIN item_text t ON t.rowid=s.rowid JOIN threads h ON h.id=t.thread_id
                 WHERE item_search MATCH ?1{filter}
                 ORDER BY instr(lower(t.text),lower(?4))>0 DESC,
                    bm25(item_search,1.0,0.5) * CASE WHEN t.kind IN ('user','assistant','agent','plan','question')
                        THEN 3.0 ELSE 1.0 END
                 LIMIT ?2 OFFSET ?3"
            )
    } else {
        // Too short for the index: a plain scan, newest first.
        values[0] = rusqlite::types::Value::Text(query.trim().to_owned());
        format!(
            "SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
                    substr(t.text,1,200)
                 FROM item_text t JOIN threads h ON h.id=t.thread_id
                 WHERE instr(lower(t.text),lower(?1))>0 AND ?4 IS NOT NULL{filter}
                 ORDER BY t.rowid DESC LIMIT ?2 OFFSET ?3"
        )
    };
    values.extend(scope_values);
    let mut hits = read_hits(conn, &sql, &values)?;
    if output && hits.len() <= limit as usize {
        // Output has no index; scan it for the plain text, newest first.
        let (filter, scope_values) = scope.sql(3);
        let mut values = vec![
            rusqlite::types::Value::Text(query.trim().to_owned()),
            rusqlite::types::Value::Integer(i64::from(limit) + 1 - i64::try_from(hits.len())?),
        ];
        values.extend(scope_values);
        let sql = format!(
            "SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
                substr(t.output,max(1,instr(lower(t.output),lower(?1))-80),240)
             FROM item_text t JOIN threads h ON h.id=t.thread_id
             WHERE instr(lower(t.output),lower(?1))>0{filter}
             ORDER BY t.rowid DESC LIMIT ?2"
        );
        let seen: std::collections::HashSet<_> =
            hits.iter().map(|hit| hit.item_id.clone()).collect();
        hits.extend(
            read_hits(conn, &sql, &values)?
                .into_iter()
                .filter(|hit| !seen.contains(&hit.item_id)),
        );
    }
    let more = hits.len() > limit as usize;
    hits.truncate(limit as usize);
    Ok(SearchPage {
        hits,
        next: more.then_some(offset + limit),
    })
}

fn read_hits(
    conn: &Connection,
    sql: &str,
    values: &[rusqlite::types::Value],
) -> Result<Vec<SearchHit>> {
    let mut statement = conn.prepare(sql)?;
    let hits = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            Ok(SearchHit {
                thread_id: ThreadId::from(row.get::<_, String>(0)?),
                thread_title: row.get(1)?,
                item_id: ItemId::from(row.get::<_, String>(2)?),
                kind: row.get(3)?,
                created_at: row.get(4)?,
                snippet: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(hits)
}

fn touching_rows(
    conn: &Connection,
    scope: &RecallScope,
    path: &str,
    limit: u32,
) -> Result<Vec<Touch>> {
    let limit = limit.clamp(1, 200);
    let (filter, scope_values) = scope.sql(3);
    let path = path.trim();
    let (condition, needle) = match fts_query(path) {
        Some(_) => (
            "t.rowid IN (SELECT rowid FROM item_search WHERE item_search MATCH ?1)",
            format!("paths : \"{}\"", path.replace('"', "\"\"")),
        ),
        None => ("instr(t.paths,?1)>0", path.to_owned()),
    };
    let mut values = vec![
        rusqlite::types::Value::Text(needle),
        rusqlite::types::Value::Integer(i64::from(limit) * 4),
    ];
    values.extend(scope_values);
    let sql = format!(
        "SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.paths,t.created_at
         FROM item_text t JOIN threads h ON h.id=t.thread_id
         WHERE t.paths<>'' AND {condition}{filter}
         ORDER BY t.rowid DESC LIMIT ?2"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Timestamp>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut touches = Vec::new();
    for (thread, title, item, paths, created_at) in rows {
        for line in paths.lines() {
            let Some((op, file)) = line.split_once('\t') else {
                continue;
            };
            if file.contains(path) {
                touches.push(Touch {
                    thread_id: ThreadId::from(thread.clone()),
                    thread_title: title.clone(),
                    item_id: ItemId::from(item.clone()),
                    op: op.to_owned(),
                    path: file.to_owned(),
                    created_at,
                });
            }
        }
        if touches.len() >= limit as usize {
            break;
        }
    }
    touches.truncate(limit as usize);
    Ok(touches)
}

impl Store {
    /// Ranked full-text search over every item's readable text; with
    /// `output`, tool output too.
    pub async fn recall_search(
        &self,
        scope: RecallScope,
        query: String,
        output: bool,
        limit: u32,
        offset: u32,
    ) -> Result<SearchPage> {
        self.read(move |conn| search_rows(conn, &scope, &query, output, limit, offset))
            .await
    }

    /// Lines matching a regex, newest items first.
    pub async fn recall_grep(&self, scope: RecallScope, query: GrepQuery) -> Result<GrepPage> {
        self.read(move |conn| grep_rows(conn, &scope, &query)).await
    }

    /// The items that read or changed a path containing `path`, newest first.
    pub async fn recall_touching(
        &self,
        scope: RecallScope,
        path: String,
        limit: u32,
    ) -> Result<Vec<Touch>> {
        self.read(move |conn| touching_rows(conn, &scope, &path, limit))
            .await
    }
}
