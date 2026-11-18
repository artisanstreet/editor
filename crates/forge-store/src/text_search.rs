use crate::Store;
use crate::error::{StorageError, operation_error};
use artisan_api::{ItemId, ProjectId, Seq, TextHit, TextSearchQuery, ThreadId, Timestamp};
use fff_grep::{LineTerminator, Match, Matcher, NoError, Searcher, Sink, SinkMatch};
use rusqlite::Connection;
use std::collections::HashMap;

type Result<T, E = StorageError> = std::result::Result<T, E>;

operation_error!(TextSearchError(StorageError), "search message text for {query:?}", query: String);

const SCAN_ROWS: usize = 60_000;
const SCAN_BYTES: usize = 32 * 1024 * 1024;
const TRIGRAM_CHARS: usize = 3;
const DEFAULT_HITS: u32 = 30;
const MAX_HITS: u32 = 100;
const HITS_PER_THREAD: usize = 3;
const SNIPPET_BEFORE: usize = 24;
const SNIPPET_CHARS: usize = 120;

struct Literal {
    lower: Vec<u8>,
}

impl Literal {
    fn new(needle: &str) -> Self {
        Self {
            lower: needle.to_ascii_lowercase().into_bytes(),
        }
    }
}

impl Matcher for Literal {
    type Error = NoError;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
        let Some((&first, rest)) = self.lower.split_first() else {
            return Ok(None);
        };
        let upper = first.to_ascii_uppercase();
        let mut from = at;
        while let Some(found) = haystack
            .get(from..)
            .and_then(|tail| memchr::memchr2(first, upper, tail))
        {
            let start = from + found;
            let end = start + self.lower.len();
            if haystack
                .get(start + 1..end)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(rest))
            {
                return Ok(Some(Match::new(start, end)));
            }
            from = start + 1;
        }
        Ok(None)
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        Some(LineTerminator::byte(b'\n'))
    }
}

struct FirstLine<'a> {
    needle: &'a Literal,
    found: Option<(usize, usize)>,
}

impl Sink for FirstLine<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, line: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        let start = usize::try_from(line.absolute_byte_offset()).unwrap_or(usize::MAX);
        self.found = self
            .needle
            .find(line.bytes())
            .ok()
            .flatten()
            .map(|found| (start + found.start(), start + found.end()));
        Ok(false)
    }
}

fn first_match(searcher: &Searcher, needle: &Literal, text: &str) -> Option<(usize, usize)> {
    let mut sink = FirstLine {
        needle,
        found: None,
    };
    searcher
        .search_slice(needle, text.as_bytes(), &mut sink)
        .ok()?;
    sink.found
        .filter(|(start, end)| text.is_char_boundary(*start) && text.is_char_boundary(*end))
}

fn snippet(text: &str, start: usize, end: usize) -> (String, (u32, u32)) {
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at);
    let from = text[line_start..start]
        .char_indices()
        .rev()
        .nth(SNIPPET_BEFORE - 1)
        .map_or(line_start, |(at, _)| line_start + at);
    let lead = if from > line_start { "…" } else { "" };
    let body: String = text[from..line_end].chars().take(SNIPPET_CHARS).collect();
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
        .then(|| format!("text : \"{}\"", wanted.replace('"', "\"\"")))
}

fn text_hits(conn: &Connection, query: &TextSearchQuery) -> Result<Vec<TextHit>> {
    let wanted = query.query.trim();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let limit = query.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS) as usize;
    let needle = Literal::new(wanted);
    let searcher = Searcher::new();
    let scan_rows = i64::try_from(SCAN_ROWS).expect("invariant: the scan bound fits in i64");
    let (mut rows, values) = match indexed_phrase(wanted) {
        Some(phrase) => (
            conn.prepare_cached(include_str!("sql/search/messages_indexed.sql"))?,
            vec![
                rusqlite::types::Value::Text(phrase),
                rusqlite::types::Value::Integer(scan_rows),
            ],
        ),
        None => (
            conn.prepare_cached(include_str!("sql/search/messages_scan.sql"))?,
            vec![rusqlite::types::Value::Integer(scan_rows)],
        ),
    };
    let mut per_thread: HashMap<ThreadId, usize> = HashMap::new();
    let mut hits = Vec::new();
    let mut scanned_bytes = 0;
    let mut scan = rows.query(rusqlite::params_from_iter(values))?;
    while let Some(row) = scan.next()? {
        if scanned_bytes >= SCAN_BYTES {
            break;
        }
        let thread_id = ThreadId::from(row.get::<_, String>(0)?);
        if per_thread.get(&thread_id).copied().unwrap_or(0) >= HITS_PER_THREAD {
            continue;
        }
        let text = row.get_ref(6)?.as_str().map_err(rusqlite::Error::from)?;
        scanned_bytes += text.len();
        let Some((start, end)) = first_match(&searcher, &needle, text) else {
            continue;
        };
        let (snippet, matched) = snippet(text, start, end);
        let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        let length = narrow(text.len());
        *per_thread.entry(thread_id.clone()).or_default() += 1;
        hits.push(TextHit {
            thread_title: row.get(4)?,
            project_id: ProjectId::from(row.get::<_, String>(5)?),
            thread_id,
            item_id: ItemId::from(row.get::<_, String>(1)?),
            position: row.get::<_, Seq>(3)?,
            created_at: row.get::<_, Timestamp>(2)?,
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
        let searched = query.query.clone();
        self.scan(move |conn| text_hits(conn, &query))
            .await
            .map_err(|cause| TextSearchError {
                query: searched,
                cause: Box::new(cause),
            })
    }
}
