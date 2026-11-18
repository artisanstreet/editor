use crate::collection::AnswerRefused;
use crate::error::StorageError;
use artisan_api::{
    ContentId, ContentQuery, ContentRange, ContentRef, EditTotals, Item, TextContent, ToolKind,
};
use forge_content::ItemTexts as _;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;
const MAX_ITEM_BYTES: usize = 512 * 1024;
const GRACE_MILLIS: i64 = 24 * 60 * 60 * 1000;

type Result<T, E = ContentError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("the text is {bytes} bytes; the limit is 64 MiB")]
    TextTooLarge { bytes: u64 },
    #[error("metadata field {key} is {bytes} bytes; the limit is 4 KiB")]
    FieldTooLong { key: String, bytes: usize },
    #[error("field {key} has {entries} entries; the limit is 256")]
    TooManyEntries { key: String, entries: usize },
    #[error("the item's metadata is {bytes} bytes; the limit is 512 KiB")]
    MetadataTooLarge { bytes: u64 },
    #[error("the item's authored and structural data is {bytes} bytes; the limit is 64 MiB")]
    DataTooLarge { bytes: u64 },
    #[error("{content} is not a content identity")]
    InvalidIdentity { content: ContentId },
    #[error("content {content} is unknown or does not match its reference")]
    UnknownReference { content: ContentId },
    #[error("content {content} is no longer stored")]
    Missing { content: ContentId },
    #[error("a range of {limit} bytes is outside 4..65536")]
    RangeLimit { limit: u32 },
    #[error("offset {offset} is past the end of content {content}, which has {total}")]
    OffsetPastEnd {
        content: ContentId,
        offset: u64,
        total: u64,
    },
    #[error("stored content {content} is damaged: {damage}")]
    Damaged { content: ContentId, damage: Damage },
    #[error(
        "a collection record is {bytes} bytes; the limit is 64 KiB, so authored text needs a source reference"
    )]
    RecordTooLarge { bytes: usize },
    #[error("a collection record's identity is empty or longer than 4096 bytes")]
    RecordIdentity,
    #[error("the collection's data is {bytes} bytes; the limit is 64 MiB")]
    CollectionTooLarge { bytes: u64 },
    #[error("collection {content} depends on itself")]
    SelfDependency { content: ContentId },
    #[error("collection dependency {content} is not stored")]
    MissingDependency { content: ContentId },
    #[error("a page of {limit} records is outside 1..64")]
    CollectionLimit { limit: u32 },
    #[error("offset {offset} is past the {total} records of collection {content}")]
    RecordOffsetPastEnd {
        content: ContentId,
        offset: u64,
        total: u64,
    },
    #[error("an answer's display text exceeds 64 MiB")]
    AnswerDisplayTooLarge,
    #[error("{questions} questions have {answers} saved answers")]
    SavedAnswerCount { questions: u64, answers: u64 },
    #[error(transparent)]
    Refused(#[from] AnswerRefused),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for ContentError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Damage {
    #[error("a chunk is not UTF-8")]
    NotUtf8,
    #[error("its chunks have a gap")]
    ChunkGap,
    #[error("a chunk is missing")]
    MissingChunk,
    #[error("a range is incomplete")]
    IncompleteRange,
    #[error("its reference does not match the stored bytes")]
    DescriptorMismatch,
    #[error("its record index is missing or inconsistent")]
    RecordIndex,
    #[error("a record is larger than a range")]
    OversizedRecord,
    #[error("a record's bytes are incomplete")]
    IncompleteRecord,
    #[error("its record index lacks a trailing record")]
    MissingTrailingRecord,
    #[error("a page did not advance")]
    StalledPage,
}

pub fn to_usize(value: u64) -> usize {
    usize::try_from(value).expect("invariant: Forge runs on 64-bit targets")
}

fn damaged(content: &ContentId, damage: Damage) -> ContentError {
    ContentError::Damaged {
        content: content.clone(),
        damage,
    }
}

pub fn normalize(conn: &Connection, item: &mut Item, previous: Option<&Item>) -> Result<()> {
    let mut bytes = 0u64;
    item.kind.try_visit_texts_mut(|text| {
        bytes = bytes.saturating_add(text.byte_len());
        if bytes > MAX_SOURCE_BYTES {
            return Err(ContentError::TextTooLarge { bytes });
        }
        normalize_text(conn, text)
    })?;
    crate::collection::normalize_item(conn, &mut item.kind)?;
    crate::collection::derive_answer_text(conn, &mut item.kind, previous.map(|item| &item.kind))?;
    if let artisan_api::ItemKind::Question {
        answer_text: Some(text),
        ..
    } = &mut item.kind
    {
        normalize_text(conn, text)?;
    }
    bytes = 0;
    item.kind.try_visit_texts(|text| {
        bytes = bytes.saturating_add(text.byte_len());
        Ok::<_, ContentError>(())
    })?;
    if let artisan_api::ItemKind::Question {
        answers: Some(artisan_api::Collection::Inline(answers)),
        ..
    } = &item.kind
    {
        for answer in answers.iter().flatten() {
            if let artisan_api::QuestionAnswer::Text { text } = answer {
                bytes = bytes.saturating_add(text.len() as u64);
            }
        }
    }
    let mut structural = 0u64;
    item.kind.try_visit_collection_sources(|source| {
        let metrics = crate::collection::source_metrics(conn, source)?;
        bytes = bytes.saturating_add(metrics.authored);
        structural = structural.saturating_add(metrics.structural);
        Ok::<_, ContentError>(())
    })?;
    let encoded = crate::read::to_value(&*item)?;
    validate_metadata(&encoded["kind"], "")?;
    let encoded_bytes = json_len(&encoded);
    if encoded_bytes > MAX_ITEM_BYTES as u64 {
        return Err(ContentError::MetadataTooLarge {
            bytes: encoded_bytes,
        });
    }
    let total = bytes
        .saturating_add(structural)
        .saturating_add(encoded_bytes);
    if total > MAX_SOURCE_BYTES {
        return Err(ContentError::DataTooLarge { bytes: total });
    }
    Ok(())
}

fn json_len(value: &serde_json::Value) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value)
        .expect("invariant: a JSON value always writes to a counter");
    count.0
}

pub fn validate_metadata(value: &serde_json::Value, key: &str) -> Result<()> {
    use serde_json::Value;
    match value {
        Value::String(text)
            if !matches!(
                key,
                "text"
                    | "description"
                    | "prompt"
                    | "result"
                    | "proposal"
                    | "caption"
                    | "message"
                    | "command"
                    | "input"
                    | "diff"
                    | "reason"
                    | "summary"
            ) =>
        {
            if text.len() > 4096 {
                return Err(ContentError::FieldTooLong {
                    key: key.to_owned(),
                    bytes: text.len(),
                });
            }
        }
        Value::Array(values) => {
            if values.len() > 256 {
                return Err(ContentError::TooManyEntries {
                    key: key.to_owned(),
                    entries: values.len(),
                });
            }
            for value in values {
                validate_metadata(value, key)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_metadata(value, key)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

pub fn normalize_text(conn: &Connection, text: &mut TextContent) -> Result<()> {
    match text {
        TextContent::Inline(value) if value.len() > artisan_api::INLINE_TEXT_BYTES => {
            let source = store_source(conn, value)?;
            *text = TextContent::Stored(source);
        }
        TextContent::Stored(source) => {
            if !source.id.is_valid() {
                return Err(ContentError::InvalidIdentity {
                    content: source.id.clone(),
                });
            }
            let known: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM content WHERE id=?1 AND bytes=?2 AND lines=?3)",
                params![source.id.as_str(), source.bytes, source.lines],
                |row| row.get(0),
            )?;
            if !known {
                return Err(ContentError::UnknownReference {
                    content: source.id.clone(),
                });
            }
        }
        TextContent::Inline(_) => {}
    }
    Ok(())
}

pub fn own(conn: &Connection, item: &Item) -> Result<()> {
    let mut next = BTreeSet::new();
    item.kind.try_visit_texts(|text| {
        if let TextContent::Stored(source) = text {
            next.insert(source.id.0.clone());
        }
        Ok::<_, ContentError>(())
    })?;
    item.kind.try_visit_collection_sources(|source| {
        next.insert(source.id.0.clone());
        Ok::<_, ContentError>(())
    })?;
    let mut query = conn.prepare_cached("SELECT content_id FROM item_content WHERE item_id=?1")?;
    let old = query
        .query_map([item.id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    for id in old.iter().filter(|id| !next.contains(*id)) {
        conn.execute(
            "DELETE FROM item_content WHERE item_id=?1 AND content_id=?2",
            params![item.id.as_str(), id],
        )?;
    }
    for id in next.into_iter().filter(|id| !old.contains(id)) {
        conn.execute(
            "INSERT INTO item_content(item_id,content_id) VALUES(?1,?2)",
            params![item.id.as_str(), id],
        )?;
    }
    Ok(())
}

pub fn range(
    conn: &Connection,
    id: &ContentId,
    query: &ContentQuery,
) -> Result<Option<ContentRange>> {
    if !id.is_valid() {
        return Err(ContentError::InvalidIdentity {
            content: id.clone(),
        });
    }
    let limit = query.limit.unwrap_or(artisan_api::CONTENT_RANGE_BYTES);
    if !(4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit) {
        return Err(ContentError::RangeLimit { limit });
    }
    let Some(total) = conn
        .query_row(
            "SELECT bytes FROM content WHERE id=?1",
            [id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    if query.offset > total {
        return Err(ContentError::OffsetPastEnd {
            content: id.clone(),
            offset: query.offset,
            total,
        });
    }
    if query.offset == total {
        return Ok(Some(ContentRange {
            id: id.clone(),
            start: total,
            end: total,
            total,
            text: String::new(),
        }));
    }
    let start_chunk: u64 = conn.query_row("SELECT start FROM content_chunks WHERE content_id=?1 AND start<=?2 ORDER BY start DESC LIMIT 1", params![id.as_str(),query.offset], |row| row.get(0))?;
    let mut read = conn.prepare_cached("SELECT start,data FROM content_chunks WHERE content_id=?1 AND start>=?2 AND start<?3 ORDER BY start")?;
    let mut rows = read.query(params![
        id.as_str(),
        start_chunk,
        query.offset.saturating_add(u64::from(limit))
    ])?;
    let mut text = String::with_capacity(limit as usize);
    let mut actual_start = query.offset;
    while let Some(row) = rows.next()? {
        let chunk_start: u64 = row.get(0)?;
        let value = row.get_ref(1)?;
        let chunk = std::str::from_utf8(value.as_blob().map_err(rusqlite::Error::from)?)
            .map_err(|_not_utf8| damaged(id, Damage::NotUtf8))?;
        let mut from = to_usize(query.offset.saturating_sub(chunk_start)).min(chunk.len());
        while !chunk.is_char_boundary(from) {
            from -= 1;
        }
        if text.is_empty() {
            actual_start = chunk_start + from as u64;
        }
        if chunk_start + from as u64 != actual_start + text.len() as u64 {
            return Err(damaged(id, Damage::ChunkGap));
        }
        let room = limit as usize - text.len();
        let mut to = (from + room).min(chunk.len());
        while !chunk.is_char_boundary(to) {
            to -= 1;
        }
        text.push_str(&chunk[from..to]);
        if to < chunk.len() || text.len() == limit as usize {
            break;
        }
    }
    if text.is_empty() {
        return Err(damaged(id, Damage::MissingChunk));
    }
    let end = actual_start + text.len() as u64;
    let expected_end = actual_start.saturating_add(u64::from(limit)).min(total);
    let complete = end <= expected_end
        && if expected_end == total {
            end == total
        } else {
            expected_end - end <= 3
        };
    if !complete {
        return Err(damaged(id, Damage::IncompleteRange));
    }
    Ok(Some(ContentRange {
        id: id.clone(),
        start: actual_start,
        end,
        total,
        text,
    }))
}

pub fn resolve(conn: &Connection, text: &TextContent) -> Result<String> {
    let source = match text {
        TextContent::Inline(text) => return Ok(text.clone()),
        TextContent::Stored(source) => source,
    };
    if source.bytes > MAX_SOURCE_BYTES {
        return Err(ContentError::TextTooLarge {
            bytes: source.bytes,
        });
    }
    let mut text = String::with_capacity(to_usize(source.bytes));
    let mut offset = 0;
    while offset < source.bytes {
        let part = range(
            conn,
            &source.id,
            &ContentQuery {
                offset,
                limit: None,
            },
        )?
        .ok_or_else(|| ContentError::Missing {
            content: source.id.clone(),
        })?;
        if !(part.total == source.bytes && part.start == offset && part.end > offset) {
            return Err(damaged(&source.id, Damage::DescriptorMismatch));
        }
        offset = part.end;
        text.push_str(&part.text);
    }
    Ok(text)
}

pub fn diff_totals(diff: &str) -> EditTotals {
    let mut totals = EditTotals::default();
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk && (line.starts_with("+++") || line.starts_with("---")) {
            continue;
        }
        if line.starts_with('+') {
            totals.added = totals.added.saturating_add(1);
        } else if line.starts_with('-') {
            totals.removed = totals.removed.saturating_add(1);
        }
    }
    totals
}

pub fn inline_edit_totals(tool: &ToolKind) -> Option<EditTotals> {
    let ToolKind::Edit { changes } = tool else {
        return None;
    };
    let mut totals = EditTotals::default();
    for change in changes.inline()? {
        let Some(diff) = &change.diff else {
            continue;
        };
        let count = diff_totals(diff.inline()?);
        totals.added = totals.added.saturating_add(count.added);
        totals.removed = totals.removed.saturating_add(count.removed);
    }
    Some(totals)
}

pub fn edit_totals(conn: &Connection, tool: &ToolKind) -> Result<Option<EditTotals>> {
    let ToolKind::Edit { changes } = tool else {
        return Ok(None);
    };
    let mut total = EditTotals::default();
    let mut offset = 0;
    loop {
        let stored_page;
        let slice = match changes {
            artisan_api::Collection::Inline(records) => records.as_slice(),
            artisan_api::Collection::Stored(reference) => {
                stored_page = crate::collection::range::<artisan_api::FileChange>(
                    conn,
                    &reference.source.id,
                    artisan_api::CollectionQuery {
                        offset,
                        limit: None,
                    },
                )?
                .ok_or_else(|| ContentError::Missing {
                    content: reference.source.id.clone(),
                })?;
                if stored_page.total != reference.count {
                    return Err(damaged(&reference.source.id, Damage::DescriptorMismatch));
                }
                stored_page.records.as_slice()
            }
        };
        for diff in slice.iter().filter_map(|change| change.diff.as_ref()) {
            let count = match diff {
                TextContent::Inline(text) => diff_totals(text),
                TextContent::Stored(source) => conn.query_row(
                    "SELECT diff_added,diff_removed FROM content WHERE id=?1",
                    [source.id.as_str()],
                    |row| {
                        Ok(EditTotals {
                            added: row.get(0)?,
                            removed: row.get(1)?,
                        })
                    },
                )?,
            };
            total.added = total.added.saturating_add(count.added);
            total.removed = total.removed.saturating_add(count.removed);
        }
        offset += slice.len() as u64;
        match changes {
            artisan_api::Collection::Inline(_) => break,
            artisan_api::Collection::Stored(reference) if offset >= reference.count => break,
            artisan_api::Collection::Stored(reference) => {
                if slice.is_empty() {
                    return Err(damaged(&reference.source.id, Damage::StalledPage));
                }
            }
        }
    }
    Ok(Some(total))
}

pub fn collect(conn: &Connection, now: i64) -> Result<usize, StorageError> {
    const BATCH_BYTES: u64 = 16 * 1024 * 1024;
    let mut query = conn.prepare_cached("SELECT id,bytes FROM content WHERE unreferenced_at IS NOT NULL AND unreferenced_at<=?1 ORDER BY unreferenced_at LIMIT 256")?;
    let mut rows = query.query([now.saturating_sub(GRACE_MILLIS)])?;
    let mut selected = Vec::new();
    let mut bytes = 0u64;
    while let Some(row) = rows.next()? {
        let size: u64 = row.get(1)?;
        if !selected.is_empty() && bytes.saturating_add(size) > BATCH_BYTES {
            break;
        }
        selected.push(row.get::<_, String>(0)?);
        bytes = bytes.saturating_add(size);
        if bytes >= BATCH_BYTES {
            break;
        }
    }
    let mut delete =
        conn.prepare_cached("DELETE FROM content WHERE id=?1 AND unreferenced_at IS NOT NULL")?;
    let mut removed = 0;
    for id in selected {
        removed += delete.execute([id])?;
    }
    Ok(removed)
}

pub fn migrate(conn: &Connection, collections: bool) -> Result<()> {
    let mut select = conn.prepare("SELECT id,body FROM items ORDER BY rowid")?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let original: String = row.get(1)?;
        let mut value: serde_json::Value = crate::read::from_body(&original)?;
        if let Some(output) = value
            .pointer_mut("/kind/output")
            .and_then(serde_json::Value::as_object_mut)
            && !output.contains_key("text")
        {
            let head = output
                .remove("head")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            let tail = output
                .remove("tail")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            let omitted = output
                .remove("truncated_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let text = if omitted == 0 {
                format!("{head}{tail}")
            } else {
                format!(
                    "{head}\n… {omitted} bytes were not retained by the earlier Forge …\n{tail}"
                )
            };
            output.insert("text".into(), text.into());
            output.insert("omitted_bytes".into(), omitted.into());
        }
        crate::schema::legacy_approval(&mut value);
        let mut item: Item = crate::read::from_value(value)?;
        if collections {
            normalize(conn, &mut item, None)?;
        } else {
            item.kind
                .try_visit_texts_mut(|text| normalize_text(conn, text))?;
        }
        let normalized = crate::read::to_body(&item)?;
        if normalized != original {
            conn.execute(
                "UPDATE items SET body=?2 WHERE id=?1",
                params![id, normalized],
            )?;
        }
        own(conn, &item)?;
    }
    Ok(())
}

pub fn store_source(conn: &Connection, value: &str) -> Result<ContentRef> {
    if value.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ContentError::TextTooLarge {
            bytes: value.len() as u64,
        });
    }
    let source = ContentRef::from_text(value);
    let totals = diff_totals(value);
    let inserted = conn.execute(
        "INSERT INTO content(id,bytes,lines,diff_added,diff_removed,unreferenced_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO NOTHING",
        params![source.id.as_str(),source.bytes,source.lines,totals.added,totals.removed,artisan_api::now()],
    )?;
    if inserted != 0 {
        let mut insert = conn
            .prepare_cached("INSERT INTO content_chunks(content_id,start,data) VALUES(?1,?2,?3)")?;
        let mut start = 0;
        while start < value.len() {
            let mut end = (start + CHUNK_BYTES).min(value.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            insert.execute(params![
                source.id.as_str(),
                start as u64,
                &value.as_bytes()[start..end]
            ])?;
            start = end;
        }
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        conn
    }
    fn stored(conn: &Connection, text: &str) -> ContentRef {
        let mut value = TextContent::Inline(text.to_owned());
        normalize_text(conn, &mut value).unwrap();
        let TextContent::Stored(source) = value else {
            panic!("large payload was not externalized")
        };
        source
    }
    #[test]
    fn ranges_reconstruct_multimegabyte_unicode_without_item_reads() {
        let conn = database();
        let text = "αβ🙂\nsource\0".repeat(800_000);
        let source = stored(&conn, &text);
        let again = stored(&conn, &text);
        assert_eq!(source, again);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        let mut rebuilt = String::new();
        let mut offset = 0;
        while offset < source.bytes {
            let part = range(
                &conn,
                &source.id,
                &ContentQuery {
                    offset,
                    limit: Some(65536),
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(part.start, offset);
            assert!(part.text.len() <= 65536);
            assert_eq!(part.end - part.start, part.text.len() as u64);
            offset = part.end;
            rebuilt.push_str(&part.text);
        }
        assert_eq!(rebuilt, text);
        for offset in [1, 3, 5, 32767, 32768, 65535] {
            let part = range(
                &conn,
                &source.id,
                &ContentQuery {
                    offset,
                    limit: Some(19),
                },
            )
            .unwrap()
            .unwrap();
            assert!(part.start <= offset && offset - part.start <= 3);
            assert_eq!(
                part.text,
                text[usize::try_from(part.start).unwrap()..usize::try_from(part.end).unwrap()]
            );
        }
        assert!(
            range(
                &conn,
                &source.id,
                &ContentQuery {
                    offset: source.bytes + 1,
                    limit: None
                }
            )
            .is_err()
        );
        assert!(
            range(
                &conn,
                &source.id,
                &ContentQuery {
                    offset: 0,
                    limit: Some(65537)
                }
            )
            .is_err()
        );
    }
    #[test]
    fn transaction_rollback_does_not_leave_chunks_and_gc_keeps_owned_sources() {
        let mut conn = database();
        {
            let tx = conn.transaction().unwrap();
            stored(&tx, &"ab".repeat(20_000));
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_chunks", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let source = stored(&conn, &"payload".repeat(4_000));
        conn.execute(
            "INSERT INTO item_content VALUES('owner',?1)",
            [source.id.as_str()],
        )
        .unwrap();
        assert_eq!(collect(&conn, i64::MAX).unwrap(), 0);
        conn.execute("DELETE FROM item_content WHERE item_id='owner'", [])
            .unwrap();
        assert_eq!(collect(&conn, artisan_api::now()).unwrap(), 0);
        assert_eq!(
            collect(&conn, artisan_api::now() + GRACE_MILLIS + 1000).unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_chunks", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn stored_edit_counts_are_not_zeroed_when_status_changes() {
        let conn = database();
        let diff = format!(
            "--- old\n+++ new\n@@ -1,1 +1,1 @@\n{}",
            "-old\n+new\n".repeat(3000)
        );
        let expected = diff_totals(&diff);
        let source = stored(&conn, &diff);
        let tool = ToolKind::Edit {
            changes: vec![artisan_api::FileChange {
                path: "a".into(),
                op: artisan_api::FileOp::Modify,
                diff: Some(TextContent::Stored(source)),
            }]
            .into(),
        };
        assert_eq!(inline_edit_totals(&tool), None);
        assert_eq!(edit_totals(&conn, &tool).unwrap(), Some(expected));
    }
    #[test]
    fn unknown_and_inconsistent_references_are_rejected_and_chunk_gaps_are_errors() {
        let conn = database();
        let source = stored(&conn, &"abcd".repeat(25000));
        let mut known = TextContent::Stored(source.clone());
        normalize_text(&conn, &mut known).unwrap();
        let mut wrong_length = source.clone();
        wrong_length.bytes += 1;
        assert!(normalize_text(&conn, &mut TextContent::Stored(wrong_length)).is_err());
        let mut unknown = source.clone();
        unknown.id = ContentId("f".repeat(64));
        assert!(normalize_text(&conn, &mut TextContent::Stored(unknown)).is_err());
        conn.execute(
            "DELETE FROM content_chunks WHERE content_id=?1 AND start=32768",
            [source.id.as_str()],
        )
        .unwrap();
        assert!(
            range(
                &conn,
                &source.id,
                &ContentQuery {
                    offset: 0,
                    limit: None
                }
            )
            .is_err()
        );
    }
    #[test]
    fn collection_batches_are_bounded_and_make_progress_on_a_large_source() {
        let conn = database();
        for (id, bytes) in [
            ("a", 20 * 1024 * 1024),
            ("b", 10 * 1024 * 1024),
            ("c", 10 * 1024 * 1024),
        ] {
            conn.execute("INSERT INTO content(id,bytes,lines,diff_added,diff_removed,unreferenced_at) VALUES(?1,?2,1,0,0,0)", params![id, bytes]).unwrap();
        }
        assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
        assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
        assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
        assert_eq!(collect(&conn, i64::MAX).unwrap(), 0);
    }
}
