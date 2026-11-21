use crate::Store;
use crate::error::AtStep as _;
use crate::error::{StorageError, operation_error};
use artisan_api::{ItemId, ProjectId, Seq, TextHit, TextSearchQuery, ThreadId, Timestamp};
use folded::Folded;
use rusqlite::Connection;
use std::collections::HashMap;

pub(crate) mod folded;

type Result<T, E = StorageError> = std::result::Result<T, E>;

operation_error!(TextSearchError(StorageError), "search message text for a {query_chars}-character query", query_chars: usize);

const SCAN_ROWS: usize = 60_000;
const SCAN_BYTES: usize = 32 * 1024 * 1024;
const TRIGRAM_CHARS: usize = 3;
const DEFAULT_HITS: u32 = 30;
const MAX_HITS: u32 = 100;
const HITS_PER_THREAD: usize = 3;
const SNIPPET_BEFORE: usize = 24;
const SNIPPET_CHARS: usize = 120;

fn snippet(text: &str, start: usize, end: usize) -> (String, (u32, u32)) {
    let (before, _) = text.split_at(start);
    let (_, after) = text.split_at(end);
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let line_end = after.find('\n').map_or(text.len(), |at| end + at);
    let (_, lead_in) = before.split_at(line_start);
    let from = lead_in
        .char_indices()
        .rev()
        .nth(SNIPPET_BEFORE - 1)
        .map_or(line_start, |(at, _)| line_start + at);
    let lead = if from > line_start { "…" } else { "" };
    let (_, shown) = text.split_at(line_end).0.split_at(from);
    let body: String = shown.chars().take(SNIPPET_CHARS).collect();
    let tail = if from + body.len() < line_end {
        "…"
    } else {
        ""
    };
    let shown_end = (end - from).min(body.len());
    let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    (
        format!("{lead}{body}{tail}"),
        (
            narrow(lead.len() + start - from),
            narrow(lead.len() + shown_end),
        ),
    )
}

fn indexed_phrase(wanted: &str) -> Option<String> {
    (wanted.chars().count() >= TRIGRAM_CHARS)
        .then(|| format!("folded : \"{}\"", wanted.replace('"', "\"\"")))
}

pub(crate) fn text_hits(conn: &Connection, query: &TextSearchQuery) -> Result<Vec<TextHit>> {
    let wanted = folded::needle(query.query.trim());
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let limit = query.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS) as usize;
    let candidate = memchr::memmem::Finder::new(wanted.as_bytes());
    let scan_rows = i64::try_from(SCAN_ROWS).expect("invariant: the scan bound fits in i64");
    let (step, mut rows, values) = match indexed_phrase(&wanted) {
        Some(phrase) => (
            "search/messages_indexed.sql",
            conn.prepare_cached(include_str!("sql/search/messages_indexed.sql"))
                .at("search/messages_indexed.sql")?,
            vec![
                rusqlite::types::Value::Text(phrase),
                rusqlite::types::Value::Integer(scan_rows),
            ],
        ),
        None => (
            "search/messages_scan.sql",
            conn.prepare_cached(include_str!("sql/search/messages_scan.sql"))
                .at("search/messages_scan.sql")?,
            vec![rusqlite::types::Value::Integer(scan_rows)],
        ),
    };
    let mut per_thread: HashMap<ThreadId, usize> = HashMap::new();
    let mut hits = Vec::new();
    let mut scanned_bytes = 0;
    let mut scan = rows.query(rusqlite::params_from_iter(values)).at(step)?;
    while let Some(row) = scan.next().at(step)? {
        if scanned_bytes >= SCAN_BYTES {
            break;
        }
        let thread_id = ThreadId::from(row.get::<_, String>(0).at(step)?);
        if per_thread.get(&thread_id).copied().unwrap_or(0) >= HITS_PER_THREAD {
            continue;
        }
        let folded = row
            .get_ref(7)
            .at(step)?
            .as_bytes()
            .map_err(rusqlite::Error::from)
            .at(step)?;
        scanned_bytes += folded.len();
        if candidate.find(folded).is_none() {
            continue;
        }
        let text = row
            .get_ref(6)
            .at(step)?
            .as_str()
            .map_err(rusqlite::Error::from)
            .at(step)?;
        let Some(std::ops::Range { start, end }) = Folded::new(text).find(&wanted) else {
            continue;
        };
        let (snippet, matched) = snippet(text, start, end);
        let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        let length = narrow(text.len());
        *per_thread.entry(thread_id.clone()).or_default() += 1;
        hits.push(TextHit {
            thread_title: row.get(4).at(step)?,
            project_id: ProjectId::from(row.get::<_, String>(5).at(step)?),
            thread_id,
            item_id: ItemId::from(row.get::<_, String>(1).at(step)?),
            position: row.get::<_, Seq>(3).at(step)?,
            created_at: row.get::<_, Timestamp>(2).at(step)?,
            snippet,
            matched,
            offset: (narrow(start), narrow(end)),
            length,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

impl Store {
    pub async fn text_search(
        &self,
        query: TextSearchQuery,
    ) -> Result<Vec<TextHit>, TextSearchError> {
        let query_chars = query.query.chars().count();
        self.scan(move |conn| text_hits(conn, &query))
            .await
            .map_err(|cause| TextSearchError {
                query_chars,
                cause: Box::new(cause),
            })
    }
}
