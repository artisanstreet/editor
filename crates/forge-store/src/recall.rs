use crate::Store;
use crate::content::ContentError;
use artisan_api::{
    ContentQuery, EngineId, FileOp, Item, ItemId, ItemKind, ProjectId, Status, TextContent,
    ThreadId, Timestamp, ToolKind,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::ops::ControlFlow;
use std::ops::Range;

type Result<T, E = ContentError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum GrepFailure {
    #[error("That pattern doesn't compile")]
    Pattern(#[source] regex::Error),
    #[error(transparent)]
    Content(#[from] ContentError),
}

crate::error::operation_error!(RecallSearchError(ContentError), "search recall for a {query_chars}-character query", query_chars: usize);
crate::error::operation_error!(RecallGrepError(GrepFailure), "grep recall for a {pattern_chars}-character pattern", pattern_chars: usize);
crate::error::operation_error!(RecallTouchingError(ContentError), "find the items touching {path:?}", path: String);

impl From<crate::StorageError> for GrepFailure {
    fn from(error: crate::StorageError) -> Self {
        Self::Content(error.into())
    }
}

impl From<rusqlite::Error> for GrepFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Content(error.into())
    }
}

const FIELD_BYTES: usize = 64 * 1024;
const FIELD_RANGE: u32 = 64 * 1024;
const GREP_SCAN_ROWS: usize = 20_000;
const PREVIEW_CHARS: usize = 400;
const PREVIEW_LEAD: usize = 100;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Searchable {
    text: String,
    output: String,
    paths: String,
}

impl Searchable {
    const fn is_empty(&self) -> bool {
        self.text.is_empty() && self.output.is_empty() && self.paths.is_empty()
    }
}

fn full(field: &str) -> ControlFlow<()> {
    if field.len() >= FIELD_BYTES {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
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
    let (kept, _) = text.split_at(text.floor_char_boundary(FIELD_BYTES - field.len()));
    field.push_str(kept);
}

fn text_of(conn: &Connection, text: &TextContent) -> Result<String> {
    match text {
        TextContent::Inline(text) => Ok(text.clone()),
        TextContent::Stored(source) => Ok(crate::content::range(
            conn,
            &source.id,
            &ContentQuery {
                offset: 0,
                limit: Some(FIELD_RANGE),
            },
        )?
        .map(|range| range.text)
        .unwrap_or_default()),
    }
}

pub(crate) fn searchable(conn: &Connection, item: &Item) -> Result<Searchable> {
    let mut found = Searchable::default();
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
            crate::collection::visit_until(conn, questions, |question| {
                push(text, &text_of(conn, &question.text)?);
                Ok(full(text))
            })?;
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
            crate::collection::visit_until(conn, steps, |step| {
                push(text, &text_of(conn, &step.text)?);
                Ok(full(text))
            })?;
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
            crate::collection::visit_until(conn, changes, |change| {
                let op = match change.op {
                    FileOp::Add => "create",
                    FileOp::Delete => "delete",
                    FileOp::Modify | FileOp::Move { .. } => "edit",
                };
                push(paths, &format!("{op}\t{}", change.path));
                Ok(full(paths))
            })?;
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
        ToolKind::Workflow { name, input } => {
            push(text, name.as_deref().unwrap_or("Workflow"));
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

fn artisan_call(item: &Item) -> bool {
    matches!(&item.kind, ItemKind::Tool { tool: ToolKind::Mcp { server, .. }, .. } if server == "artisan")
}

pub(crate) fn index(conn: &Connection, item: &Item) -> Result<()> {
    let rowid: i64 = conn
        .prepare_cached(include_str!("sql/recall/rowid.sql"))?
        .query_row([item.id.as_str()], |row| row.get(0))?;
    let next = searchable(conn, item)?;
    let previous = conn
        .prepare_cached(include_str!("sql/recall/indexed.sql"))?
        .query_row([rowid], |row| {
            Ok((
                Searchable {
                    text: row.get(0)?,
                    output: row.get(1)?,
                    paths: row.get(2)?,
                },
                row.get::<_, String>(3)?,
            ))
        })
        .optional()?;
    if previous.as_ref().map(|(searchable, _)| searchable) == Some(&next) {
        return Ok(());
    }
    if let Some((previous, folded)) = &previous {
        conn.prepare_cached(include_str!("sql/recall/unindex.sql"))?
            .execute(params![rowid, previous.text, previous.paths])?;
        conn.prepare_cached(include_str!("sql/search/unindex.sql"))?
            .execute(params![rowid, folded])?;
    }
    if next.is_empty() {
        conn.prepare_cached(include_str!("sql/recall/delete_text.sql"))?
            .execute([rowid])?;
        return Ok(());
    }
    let kind = RecallKind::of(&item.kind);
    let folded = if kind.is_message() {
        crate::text_search::folded::text(&next.text)
    } else {
        String::new()
    };
    conn.prepare_cached(include_str!("sql/recall/save_text.sql"))?
        .execute(params![
            rowid,
            item.thread_id.as_str(),
            item.id.as_str(),
            kind.name(),
            item.created_at,
            next.text,
            next.output,
            next.paths,
            folded,
        ])?;
    conn.prepare_cached(include_str!("sql/recall/index.sql"))?
        .execute(params![rowid, next.text, next.paths])?;
    conn.prepare_cached(include_str!("sql/search/index.sql"))?
        .execute(params![rowid, folded])?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallKind {
    User,
    Assistant,
    Tool,
    Agent,
    Question,
    Plan,
    Image,
    Compaction,
    Notice,
    Error,
}

impl RecallKind {
    const fn of(kind: &ItemKind) -> Self {
        match kind {
            ItemKind::UserMessage { .. } => Self::User,
            ItemKind::AssistantMessage { .. } => Self::Assistant,
            ItemKind::Tool { .. } => Self::Tool,
            ItemKind::Agent { .. } => Self::Agent,
            ItemKind::Question { .. } => Self::Question,
            ItemKind::Plan { .. } => Self::Plan,
            ItemKind::Image { .. } => Self::Image,
            ItemKind::Compaction { .. } => Self::Compaction,
            ItemKind::Notice { .. } => Self::Notice,
            ItemKind::Error { .. } => Self::Error,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Agent => "agent",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Image => "image",
            Self::Compaction => "compaction",
            Self::Notice => "notice",
            Self::Error => "error",
        }
    }

    const fn is_message(self) -> bool {
        match self {
            Self::User | Self::Assistant => true,
            Self::Tool
            | Self::Agent
            | Self::Question
            | Self::Plan
            | Self::Image
            | Self::Compaction
            | Self::Notice
            | Self::Error => false,
        }
    }

    const ALL: [Self; 10] = [
        Self::User,
        Self::Assistant,
        Self::Tool,
        Self::Agent,
        Self::Question,
        Self::Plan,
        Self::Image,
        Self::Compaction,
        Self::Notice,
        Self::Error,
    ];
}

impl rusqlite::types::FromSql for RecallKind {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let name = value.as_str()?;
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
            .ok_or_else(|| {
                rusqlite::types::FromSqlError::Other(format!("unknown recall kind {name:?}").into())
            })
    }
}

const INDEX_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum RefreshIndexError {
    #[error("read the recall index version")]
    Version(#[source] rusqlite::Error),
    #[error("rebuild the recall index at version {version}")]
    Rebuild {
        version: i64,
        #[source]
        source: ContentError,
    },
}

fn index_version(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    let present: bool = conn.query_row(
        include_str!("sql/recall/has_index_version.sql"),
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(None);
    }
    conn.query_row(include_str!("sql/recall/index_version.sql"), [], |row| {
        row.get(0)
    })
    .optional()
}

pub(crate) fn refresh_index(conn: &mut Connection) -> Result<(), RefreshIndexError> {
    let built = index_version(conn).map_err(RefreshIndexError::Version)?;
    if built == Some(INDEX_VERSION) {
        return Ok(());
    }
    tracing::info!(
        built,
        version = INDEX_VERSION,
        "rebuilding the recall index"
    );
    let rebuild = |source| RefreshIndexError::Rebuild {
        version: INDEX_VERSION,
        source,
    };
    let sql = |source: rusqlite::Error| rebuild(source.into());
    let tx = conn.transaction().map_err(sql)?;
    tx.execute_batch(include_str!("sql/recall/rebuild.sql"))
        .map_err(sql)?;
    backfill(&tx).map_err(rebuild)?;
    tx.execute(
        include_str!("sql/recall/set_index_version.sql"),
        [INDEX_VERSION],
    )
    .map_err(sql)?;
    tx.commit().map_err(sql)
}

fn backfill(conn: &Connection) -> Result<()> {
    let mut rows = conn.prepare(include_str!("sql/recall/items.sql"))?;
    let bodies = rows.query_map([], |row| row.get::<_, String>(0))?;
    let mut skipped = 0_u64;
    for body in bodies {
        let body = body?;
        let Ok(item) = serde_json::from_str::<Item>(&body) else {
            skipped += 1;
            continue;
        };
        index(conn, &item)?;
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            "items left out of recall: their bodies didn't read"
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallScope {
    pub thread: Option<ThreadId>,
    pub project: Option<ProjectId>,
    pub engine: Option<EngineId>,
    #[serde(default)]
    pub kinds: Vec<RecallKind>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    #[serde(default)]
    pub skip_archived: bool,
}

impl RecallScope {
    fn sql(&self, first: usize) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut clauses = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        let mut add = |clause: &str, value: Value| {
            clauses.push(
                clause
                    .trim_end()
                    .replace('?', &format!("?{}", first + values.len())),
            );
            values.push(value);
        };
        if let Some(thread) = &self.thread {
            add(
                include_str!("sql/recall/scope/thread.sql"),
                Value::Text(thread.as_str().to_owned()),
            );
        }
        if let Some(project) = &self.project {
            add(
                include_str!("sql/recall/scope/project.sql"),
                Value::Text(project.as_str().to_owned()),
            );
        }
        if let Some(engine) = self.engine {
            add(
                include_str!("sql/recall/scope/engine.sql"),
                Value::Text(engine.as_str().to_owned()),
            );
        }
        if let Some(since) = self.since {
            add(
                include_str!("sql/recall/scope/since.sql"),
                Value::Integer(since),
            );
        }
        if let Some(until) = self.until {
            add(
                include_str!("sql/recall/scope/until.sql"),
                Value::Integer(until),
            );
        }
        if !self.kinds.is_empty() {
            let list =
                serde_json::to_string(&self.kinds).expect("invariant: kind names always encode");
            add(
                include_str!("sql/recall/scope/kinds.sql"),
                Value::Text(list),
            );
        }
        if self.skip_archived {
            clauses.push(
                include_str!("sql/recall/scope/unarchived.sql")
                    .trim_end()
                    .to_owned(),
            );
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
    pub kind: RecallKind,
    pub created_at: Timestamp,
    pub snippet: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum SearchCursor {
    Text { offset: u32 },
    Output { offset: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum SearchCursorError {
    #[error("{cursor:?} is not a search cursor from a previous page")]
    Shape { cursor: String },
    #[error("search cursor {cursor:?} has no offset")]
    Offset {
        cursor: String,
        #[source]
        cause: std::num::ParseIntError,
    },
}

impl std::fmt::Display for SearchCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { offset } => write!(f, "text:{offset}"),
            Self::Output { offset } => write!(f, "output:{offset}"),
        }
    }
}

impl std::str::FromStr for SearchCursor {
    type Err = SearchCursorError;

    fn from_str(cursor: &str) -> Result<Self, Self::Err> {
        let refused = || SearchCursorError::Shape {
            cursor: cursor.to_owned(),
        };
        let (phase, offset) = cursor.split_once(':').ok_or_else(refused)?;
        let offset = offset.parse().map_err(|cause| SearchCursorError::Offset {
            cursor: cursor.to_owned(),
            cause,
        })?;
        match phase {
            "text" => Ok(Self::Text { offset }),
            "output" => Ok(Self::Output { offset }),
            _ => Err(refused()),
        }
    }
}

impl From<SearchCursor> for String {
    fn from(cursor: SearchCursor) -> Self {
        cursor.to_string()
    }
}

impl TryFrom<String> for SearchCursor {
    type Error = SearchCursorError;

    fn try_from(cursor: String) -> Result<Self, Self::Error> {
        cursor.parse()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub next: Option<SearchCursor>,
}

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
    pub kind: RecallKind,
    pub created_at: Timestamp,
    pub field: GrepField,
    pub line: u32,
    pub before: Vec<String>,
    pub text: String,
    pub after: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrepField {
    Text,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum GrepCursor {
    Before {
        row: i64,
    },
    At {
        row: i64,
        field: GrepField,
        line: u32,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GrepCursorError {
    #[error("{cursor:?} is not a grep cursor from a previous page")]
    Shape { cursor: String },
    #[error("grep cursor {cursor:?} has no row or line number")]
    Number {
        cursor: String,
        #[source]
        cause: std::num::ParseIntError,
    },
}

impl GrepCursor {
    const fn last_row(self) -> i64 {
        match self {
            Self::Before { row } => row - 1,
            Self::At { row, .. } => row,
        }
    }

    fn skips(self, row: i64, field: GrepField, line: u32) -> bool {
        match self {
            Self::Before { .. } => false,
            Self::At {
                row: at,
                field: from,
                line: first,
            } => row == at && field.cmp(&from).then(line.cmp(&first)).is_lt(),
        }
    }
}

impl std::fmt::Display for GrepCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Before { row } => write!(f, "{row}"),
            Self::At {
                row,
                field: GrepField::Text,
                line,
            } => write!(f, "{row}:text:{line}"),
            Self::At {
                row,
                field: GrepField::Output,
                line,
            } => write!(f, "{row}:output:{line}"),
        }
    }
}

impl std::str::FromStr for GrepCursor {
    type Err = GrepCursorError;

    fn from_str(cursor: &str) -> Result<Self, Self::Err> {
        let refused = || GrepCursorError::Shape {
            cursor: cursor.to_owned(),
        };
        let number = |cause| GrepCursorError::Number {
            cursor: cursor.to_owned(),
            cause,
        };
        let mut parts = cursor.split(':');
        let row = parts.next().ok_or_else(refused)?.parse().map_err(number)?;
        match (parts.next(), parts.next(), parts.next()) {
            (None, None, None) => Ok(Self::Before { row }),
            (Some(field), Some(line), None) => Ok(Self::At {
                row,
                field: match field {
                    "text" => GrepField::Text,
                    "output" => GrepField::Output,
                    _ => return Err(refused()),
                },
                line: line.parse().map_err(number)?,
            }),
            _ => Err(refused()),
        }
    }
}

impl From<GrepCursor> for String {
    fn from(cursor: GrepCursor) -> Self {
        cursor.to_string()
    }
}

impl TryFrom<String> for GrepCursor {
    type Error = GrepCursorError;

    fn try_from(cursor: String) -> Result<Self, Self::Error> {
        cursor.parse()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepPage {
    pub matches: Vec<GrepMatch>,
    pub next: Option<GrepCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepQuery {
    pub pattern: String,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub output: bool,
    #[serde(default)]
    pub context: u32,
    pub max_matches: Option<u32>,
    pub cursor: Option<GrepCursor>,
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

fn preview(line: &str, found: Option<Range<usize>>) -> String {
    let start = found.map_or(0, |found| {
        let (before, _) = line.split_at(line.floor_char_boundary(found.start));
        before
            .char_indices()
            .rev()
            .take(PREVIEW_LEAD)
            .last()
            .map_or(before.len(), |(at, _)| at)
    });
    let (_, shown) = line.split_at(start);
    let body: String = shown.chars().take(PREVIEW_CHARS).collect();
    format!(
        "{}{body}{}",
        if start > 0 { "…" } else { "" },
        if body.len() < shown.len() { "…" } else { "" }
    )
}

fn grep_rows(
    conn: &Connection,
    scope: &RecallScope,
    query: &GrepQuery,
) -> Result<GrepPage, GrepFailure> {
    let regex = regex::RegexBuilder::new(&query.pattern)
        .case_insensitive(query.ignore_case)
        .size_limit(1 << 20)
        .build()
        .map_err(GrepFailure::Pattern)?;
    let max = query.max_matches.unwrap_or(50).clamp(1, 500) as usize;
    let context = query.context.min(10) as usize;
    let (filter, mut values) = scope.sql(2);
    let resume = query.cursor.unwrap_or(GrepCursor::Before { row: i64::MAX });
    values.insert(0, rusqlite::types::Value::Integer(resume.last_row()));
    let output = if query.output { "t.output" } else { "''" };
    let sql = include_str!("sql/recall/grep.sql")
        .replace("__OUTPUT__", output)
        .replace("__SCOPE__", &filter)
        .replace("__LIMIT__", &(GREP_SCAN_ROWS + 1).to_string());
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
                next: last.map(|row| GrepCursor::Before { row }),
            });
        }
        let rowid: i64 = row.get(0)?;
        last = Some(rowid);
        let thread_id = ThreadId::from(row.get::<_, String>(1)?);
        let thread_title: Option<String> = row.get(2)?;
        let item_id = ItemId::from(row.get::<_, String>(3)?);
        let kind: RecallKind = row.get(4)?;
        let created_at: Timestamp = row.get(7)?;
        for (field, body) in [
            (GrepField::Text, row.get::<_, String>(5)?),
            (GrepField::Output, row.get(6)?),
        ] {
            if !regex.is_match(&body) {
                continue;
            }
            let lines: Vec<&str> = body.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
                if resume.skips(rowid, field, number) {
                    continue;
                }
                let Some(found) = regex.find(line) else {
                    continue;
                };
                let around = |line: &&str| preview(line, None);
                matches.push(GrepMatch {
                    thread_id: thread_id.clone(),
                    thread_title: thread_title.clone(),
                    item_id: item_id.clone(),
                    kind,
                    created_at,
                    field,
                    line: number,
                    before: lines[index.saturating_sub(context)..index]
                        .iter()
                        .map(around)
                        .collect(),
                    text: preview(line, Some(found.range())),
                    after: lines[index + 1..(index + 1 + context).min(lines.len())]
                        .iter()
                        .map(around)
                        .collect(),
                });
                if matches.len() >= max {
                    return Ok(GrepPage {
                        matches,
                        next: Some(GrepCursor::At {
                            row: rowid,
                            field,
                            line: number.saturating_add(1),
                        }),
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

struct TextMatch {
    condition: &'static str,
    needle: String,
}

impl TextMatch {
    fn of(query: &str) -> Self {
        match fts_query(query) {
            Some(fts) => Self {
                condition: include_str!("sql/recall/match/indexed.sql").trim_end(),
                needle: fts,
            },
            None => Self {
                condition: include_str!("sql/recall/match/text.sql").trim_end(),
                needle: query.trim().to_owned(),
            },
        }
    }
}

fn text_hits(
    conn: &Connection,
    scope: &RecallScope,
    query: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<SearchHit>> {
    let (filter, scope_values) = scope.sql(5);
    let mut values = vec![
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Integer(i64::from(limit)),
        rusqlite::types::Value::Integer(i64::from(offset)),
        rusqlite::types::Value::Text(query.trim().to_owned()),
    ];
    let sql = if let Some(fts) = fts_query(query) {
        values[0] = rusqlite::types::Value::Text(fts);
        include_str!("sql/recall/text_indexed.sql").replace("__SCOPE__", &filter)
    } else {
        values[0] = rusqlite::types::Value::Text(query.trim().to_owned());
        include_str!("sql/recall/text_scan.sql").replace("__SCOPE__", &filter)
    };
    values.extend(scope_values);
    read_hits(conn, &sql, &values)
}

fn output_hits(
    conn: &Connection,
    scope: &RecallScope,
    query: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<SearchHit>> {
    let text = TextMatch::of(query);
    let (filter, scope_values) = scope.sql(5);
    let mut values = vec![
        rusqlite::types::Value::Text(text.needle),
        rusqlite::types::Value::Integer(i64::from(limit)),
        rusqlite::types::Value::Integer(i64::from(offset)),
        rusqlite::types::Value::Text(query.trim().to_owned()),
    ];
    values.extend(scope_values);
    let sql = include_str!("sql/recall/output.sql")
        .replace("__TEXT__", text.condition)
        .replace("__SCOPE__", &filter);
    read_hits(conn, &sql, &values)
}

fn search_rows(
    conn: &Connection,
    scope: &RecallScope,
    query: &str,
    output: bool,
    limit: u32,
    cursor: Option<SearchCursor>,
) -> Result<SearchPage> {
    let limit = limit.clamp(1, 100);
    let page = |mut hits: Vec<SearchHit>, next: SearchCursor| {
        let more = hits.len() > limit as usize;
        hits.truncate(limit as usize);
        SearchPage {
            hits,
            next: more.then_some(next),
        }
    };
    match cursor.unwrap_or(SearchCursor::Text { offset: 0 }) {
        SearchCursor::Text { offset } => {
            let mut hits = text_hits(conn, scope, query, limit + 1, offset)?;
            if hits.len() > limit as usize || !output {
                return Ok(page(
                    hits,
                    SearchCursor::Text {
                        offset: offset + limit,
                    },
                ));
            }
            let room = limit - u32::try_from(hits.len()).expect("invariant: at most `limit` hits");
            hits.extend(output_hits(conn, scope, query, room + 1, 0)?);
            Ok(page(hits, SearchCursor::Output { offset: room }))
        }
        SearchCursor::Output { offset } => Ok(page(
            output_hits(conn, scope, query, limit + 1, offset)?,
            SearchCursor::Output {
                offset: offset + limit,
            },
        )),
    }
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
            include_str!("sql/recall/match/indexed.sql").trim_end(),
            format!("paths : \"{}\"", path.replace('"', "\"\"")),
        ),
        None => (
            include_str!("sql/recall/match/paths.sql").trim_end(),
            path.to_owned(),
        ),
    };
    let mut values = vec![
        rusqlite::types::Value::Text(needle),
        rusqlite::types::Value::Integer(i64::from(limit) * 4),
    ];
    values.extend(scope_values);
    let sql = include_str!("sql/recall/touching.sql")
        .replace("__PATHS__", condition)
        .replace("__SCOPE__", &filter);
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
    pub async fn recall_search(
        &self,
        scope: RecallScope,
        query: String,
        output: bool,
        limit: u32,
        cursor: Option<SearchCursor>,
    ) -> Result<SearchPage, RecallSearchError> {
        let query_chars = query.chars().count();
        self.scan(move |conn| search_rows(conn, &scope, &query, output, limit, cursor))
            .await
            .map_err(|cause| RecallSearchError {
                query_chars,
                cause: Box::new(cause),
            })
    }

    pub async fn recall_grep(
        &self,
        scope: RecallScope,
        query: GrepQuery,
    ) -> Result<GrepPage, RecallGrepError> {
        let pattern_chars = query.pattern.chars().count();
        self.scan(move |conn| grep_rows(conn, &scope, &query))
            .await
            .map_err(|cause| RecallGrepError {
                pattern_chars,
                cause: Box::new(cause),
            })
    }

    pub async fn recall_touching(
        &self,
        scope: RecallScope,
        path: String,
        limit: u32,
    ) -> Result<Vec<Touch>, RecallTouchingError> {
        let touched = path.clone();
        self.scan(move |conn| touching_rows(conn, &scope, &path, limit))
            .await
            .map_err(|cause| RecallTouchingError {
                path: touched,
                cause: Box::new(cause),
            })
    }
}
