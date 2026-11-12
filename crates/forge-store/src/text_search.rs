use crate::Store;
use anyhow::{Context, Result};
use artisan_api::{ItemId, ProjectId, Seq, TextHit, TextSearchQuery, ThreadId, Timestamp};
use fff_grep::{LineTerminator, Match, Matcher, NoError, Searcher, Sink, SinkMatch};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

const SCAN_ROWS: usize = 60_000;
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
    let before: Vec<(usize, char)> = text[line_start..start].char_indices().collect();
    let from = before
        .len()
        .checked_sub(SNIPPET_BEFORE)
        .filter(|skipped| *skipped > 0)
        .map_or(line_start, |skipped| line_start + before[skipped].0);
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

struct Home {
    title: Option<String>,
    project: ProjectId,
    listed: bool,
}

fn home(conn: &Connection, thread: &ThreadId) -> Result<Option<Home>> {
    conn.query_row(
        "SELECT json_extract(body,'$.title'),json_extract(body,'$.project_id'),
                COALESCE(json_extract(body,'$.archived'),0),json_extract(body,'$.parent_thread_id')
         FROM threads WHERE id=?1",
        params![thread.as_str()],
        |row| {
            Ok(Home {
                title: row.get(0)?,
                project: ProjectId::from(row.get::<_, String>(1)?),
                listed: !row.get::<_, bool>(2)? && row.get::<_, Option<String>>(3)?.is_none(),
            })
        },
    )
    .optional()
    .with_context(|| format!("read thread {thread} for a text hit"))
}

fn text_hits(conn: &Connection, query: &TextSearchQuery) -> Result<Vec<TextHit>> {
    let wanted = query.query.trim();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let limit = query.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS) as usize;
    let needle = Literal::new(wanted);
    let searcher = Searcher::new();
    let mut rows = conn.prepare_cached(&format!(
        "SELECT thread_id,item_id,created_at,text FROM item_text
         WHERE kind IN ('user','assistant') ORDER BY rowid DESC LIMIT {SCAN_ROWS}"
    ))?;
    let mut place = conn.prepare_cached(
        "SELECT position FROM items WHERE id=?1 AND parent_id IS NULL AND work=0",
    )?;
    let mut homes: HashMap<ThreadId, Option<Home>> = HashMap::new();
    let mut per_thread: HashMap<ThreadId, usize> = HashMap::new();
    let mut hits = Vec::new();
    let mut scan = rows.query([])?;
    while let Some(row) = scan.next()? {
        let text: String = row.get(3)?;
        let Some((start, end)) = first_match(&searcher, &needle, &text) else {
            continue;
        };
        let thread_id = ThreadId::from(row.get::<_, String>(0)?);
        if per_thread.get(&thread_id).copied().unwrap_or(0) >= HITS_PER_THREAD {
            continue;
        }
        if !homes.contains_key(&thread_id) {
            let found = home(conn, &thread_id)?;
            homes.insert(thread_id.clone(), found);
        }
        let known = &homes[&thread_id];
        let Some(home) = known.as_ref().filter(|home| home.listed) else {
            continue;
        };
        let item_id = ItemId::from(row.get::<_, String>(1)?);
        let Some(position) = place
            .query_row(params![item_id.as_str()], |row| row.get::<_, Seq>(0))
            .optional()?
        else {
            continue;
        };
        let created_at: Timestamp = row.get(2)?;
        let (snippet, matched) = snippet(&text, start, end);
        let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        *per_thread.entry(thread_id.clone()).or_default() += 1;
        hits.push(TextHit {
            thread_title: home.title.clone(),
            project_id: home.project.clone(),
            thread_id,
            item_id,
            position,
            created_at,
            snippet,
            matched,
            offset: (narrow(start), narrow(end)),
            length: narrow(text.len()),
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

impl Store {
    pub async fn text_search(&self, query: TextSearchQuery) -> Result<Vec<TextHit>> {
        self.scan(move |conn| {
            text_hits(conn, &query)
                .with_context(|| format!("search message text for {:?}", query.query))
        })
        .await
    }
}
