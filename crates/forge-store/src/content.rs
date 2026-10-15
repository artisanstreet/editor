//! Immutable UTF-8 sources. Item writes, chunk insertion and ownership changes
//! share the existing writer transaction; range reads never deserialize an Item.
use anyhow::{Context, Result, ensure};
use artisan_api::{
    ContentId, ContentQuery, ContentRange, ContentRef, EditTotals, Item, TextContent, ToolKind,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

pub(crate) const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ITEM_BYTES: usize = 512 * 1024;
const GRACE_MILLIS: i64 = 24 * 60 * 60 * 1000;

pub(crate) fn normalize(conn: &Connection, item: &mut Item, previous: Option<&Item>) -> Result<()> {
    let mut bytes = 0u64;
    item.kind.try_visit_texts_mut(|text| {
        bytes = bytes.saturating_add(text.byte_len());
        ensure!(bytes <= MAX_SOURCE_BYTES, "item text exceeds 64 MiB");
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
        Ok::<_, anyhow::Error>(())
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
        Ok::<_, anyhow::Error>(())
    })?;
    let body = serde_json::to_value(&item.kind)?;
    validate_metadata(&body, "")?;
    ensure!(
        serde_json::to_vec(item)?.len() <= MAX_ITEM_BYTES,
        "item metadata exceeds 512 KiB"
    );
    ensure!(
        bytes
            .saturating_add(structural)
            .saturating_add(serde_json::to_vec(item)?.len() as u64)
            <= MAX_SOURCE_BYTES,
        "item authored and structural data exceeds 64 MiB"
    );
    Ok(())
}

pub(crate) fn validate_metadata(value: &serde_json::Value, key: &str) -> Result<()> {
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
            ) =>
        {
            ensure!(
                text.len() <= 4096,
                "item metadata field {key} exceeds 4 KiB"
            );
        }
        Value::Array(values) => {
            ensure!(
                values.len() <= 256,
                "item field {key} contains more than 256 entries"
            );
            for value in values {
                validate_metadata(value, key)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_metadata(value, key)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn normalize_text(conn: &Connection, text: &mut TextContent) -> Result<()> {
    match text {
        TextContent::Inline(value) if value.len() > artisan_api::INLINE_TEXT_BYTES => {
            ensure!(
                value.len() as u64 <= MAX_SOURCE_BYTES,
                "source exceeds 64 MiB"
            );
            let source = store_source(conn, value)?;
            *text = TextContent::Stored(source);
        }
        TextContent::Stored(source) => {
            ensure!(source.id.is_valid(), "invalid content identity");
            let known: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM content WHERE id=?1 AND bytes=?2 AND lines=?3)",
                params![source.id.as_str(), source.bytes, source.lines],
                |row| row.get(0),
            )?;
            ensure!(known, "content reference is unavailable or inconsistent");
        }
        TextContent::Inline(_) => {}
    }
    Ok(())
}

pub(crate) fn own(conn: &Connection, item: &Item) -> Result<()> {
    let mut next = BTreeSet::new();
    item.kind.try_visit_texts(|text| {
        if let TextContent::Stored(source) = text {
            next.insert(source.id.0.clone());
        }
        Ok::<_, anyhow::Error>(())
    })?;
    item.kind.try_visit_collection_sources(|source| {
        next.insert(source.id.0.clone());
        Ok::<_, anyhow::Error>(())
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

pub(crate) fn range(
    conn: &Connection,
    id: &ContentId,
    query: ContentQuery,
) -> Result<Option<ContentRange>> {
    ensure!(id.is_valid(), "invalid content identity");
    let limit = query.limit.unwrap_or(artisan_api::CONTENT_RANGE_BYTES);
    ensure!(
        (4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit),
        "content range limit must be 4..65536 bytes"
    );
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
    ensure!(
        query.offset <= total,
        "content offset exceeds source length"
    );
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
        let chunk = std::str::from_utf8(value.as_blob()?).context("stored content is not UTF-8")?;
        let mut from = usize::try_from(query.offset.saturating_sub(chunk_start))?.min(chunk.len());
        while !chunk.is_char_boundary(from) {
            from -= 1;
        }
        if text.is_empty() {
            actual_start = chunk_start + from as u64;
        }
        ensure!(
            chunk_start + from as u64 == actual_start + text.len() as u64,
            "stored source has a gap between chunks"
        );
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
    ensure!(!text.is_empty(), "stored source has a missing chunk");
    let end = actual_start + text.len() as u64;
    let expected_end = actual_start.saturating_add(u64::from(limit)).min(total);
    ensure!(
        end <= expected_end
            && if expected_end == total {
                end == total
            } else {
                expected_end - end <= 3
            },
        "stored source has an incomplete range"
    );
    Ok(Some(ContentRange {
        id: id.clone(),
        start: actual_start,
        end,
        total,
        text,
    }))
}

pub(crate) fn resolve(conn: &Connection, text: &TextContent) -> Result<String> {
    let TextContent::Stored(source) = text else {
        return Ok(text.inline().expect("inline variant").to_owned());
    };
    ensure!(
        source.bytes <= MAX_SOURCE_BYTES,
        "source exceeds admitted resolver budget"
    );
    let mut text = String::with_capacity(usize::try_from(source.bytes)?);
    let mut offset = 0;
    while offset < source.bytes {
        let part = range(
            conn,
            &source.id,
            ContentQuery {
                offset,
                limit: None,
            },
        )?
        .context("source is no longer available")?;
        ensure!(
            part.total == source.bytes && part.start == offset && part.end > offset,
            "source descriptor mismatch"
        );
        offset = part.end;
        text.push_str(&part.text);
    }
    Ok(text)
}

pub(crate) fn edit_totals(conn: &Connection, tool: &ToolKind) -> Result<Option<EditTotals>> {
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
                .context("edit collection is missing")?;
                ensure!(
                    stored_page.total == reference.count,
                    "edit collection descriptor mismatch"
                );
                stored_page.records.as_slice()
            }
        };
        for diff in slice.iter().filter_map(|change| change.diff.as_ref()) {
            let count = match diff {
                TextContent::Inline(text) => EditTotals::from_diff(text),
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
            _ => ensure!(!slice.is_empty(), "edit collection page did not advance"),
        }
    }
    Ok(Some(total))
}

pub(crate) fn collect(conn: &Connection, now: i64) -> Result<usize> {
    // Superseded sources remain readable for a full day for in-flight windows and
    // exports. Current owners never expire. A failed old request gets an explicit 404.
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
        // One admitted source may exceed the batch target, up to 64 MiB.
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

/// A one-time conversion preserves Item IDs, positions and revisions. It never
/// emits events or rewrites an Item repeatedly for its individual payload fields.
pub(crate) fn migrate(conn: &Connection, collections: bool) -> Result<()> {
    let mut select = conn.prepare("SELECT id,body,native_id FROM items ORDER BY rowid")?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let original: String = row.get(1)?;
        let mut value: serde_json::Value = serde_json::from_str(&original)?;
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
        let mut item: Item =
            serde_json::from_value(value).with_context(|| format!("migrate item source {id}"))?;
        item.native_id = row.get(2)?;
        if collections {
            normalize(conn, &mut item, None)?;
        } else {
            item.kind
                .try_visit_texts_mut(|text| normalize_text(conn, text))?;
        }
        let normalized = serde_json::to_string(&item)?;
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
                ContentQuery {
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
                ContentQuery {
                    offset,
                    limit: Some(19),
                },
            )
            .unwrap()
            .unwrap();
            assert!(part.start <= offset && offset - part.start <= 3);
            assert_eq!(part.text, text[part.start as usize..part.end as usize]);
        }
        assert!(
            range(
                &conn,
                &source.id,
                ContentQuery {
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
                ContentQuery {
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
        let expected = EditTotals::from_diff(&diff);
        let source = stored(&conn, &diff);
        let tool = ToolKind::Edit {
            changes: vec![artisan_api::FileChange {
                path: "a".into(),
                op: artisan_api::FileOp::Modify,
                diff: Some(TextContent::Stored(source)),
            }]
            .into(),
        };
        assert_eq!(tool.edit_totals(), None);
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
                ContentQuery {
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

/// The one immutable source insertion path, also used for derived parser metadata.
pub(crate) fn store_source(conn: &Connection, value: &str) -> Result<ContentRef> {
    ensure!(
        value.len() as u64 <= MAX_SOURCE_BYTES,
        "source exceeds 64 MiB"
    );
    let source = ContentRef::from_text(value);
    let totals = EditTotals::from_diff(value);
    let inserted = conn.execute(
        "INSERT INTO content(id,bytes,lines,diff_added,diff_removed,unreferenced_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO NOTHING",
        params![source.id.as_str(),source.bytes,source.lines,totals.added,totals.removed,artisan_api::now()],
    )?;
    if inserted != 0 {
        let mut insert = conn
            .prepare_cached("INSERT INTO content_chunks(content_id,start,data) VALUES(?1,?2,?3)")?;
        let mut start = 0;
        while start < value.len() {
            let mut end = (start + artisan_api::CONTENT_CHUNK_BYTES).min(value.len());
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
