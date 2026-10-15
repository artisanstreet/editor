//! Immutable derived Markdown topology attached to the existing source owner.
//! No Item, harness, rendering style or pixel layout participates in this scan.
use anyhow::{Result, ensure};
use artisan_api::{
    ContentId, ContentRef, SourceIndexFormat, SourceIndexHeader, SourceIndexQuery,
    SourceIndexRange, SourceLeafDescriptor, TextContent,
};
use rusqlite::{Connection, OptionalExtension, params};
const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const SCHEMA: &str = r"
CREATE TABLE source_indexes(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL,
 metadata_id TEXT NOT NULL, leaf_count INTEGER NOT NULL, PRIMARY KEY(source_id,format,version)) WITHOUT ROWID;
CREATE TABLE source_index_leaves(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 source_start INTEGER NOT NULL, source_end INTEGER NOT NULL, metadata_start INTEGER NOT NULL, metadata_end INTEGER NOT NULL,
 PRIMARY KEY(source_id,format,version,ordinal)) WITHOUT ROWID;
CREATE TRIGGER source_index_delete AFTER DELETE ON content BEGIN
 DELETE FROM source_index_leaves WHERE source_id=OLD.id;
 DELETE FROM source_indexes WHERE source_id=OLD.id;
END;
";

pub(crate) fn header(
    conn: &Connection,
    id: &ContentId,
    format: SourceIndexFormat,
) -> Result<Option<SourceIndexHeader>> {
    ensure!(id.is_valid(), "invalid source identity");
    conn.query_row("SELECT s.bytes,s.lines,m.id,m.bytes,m.lines,i.leaf_count FROM source_indexes i JOIN content s ON s.id=i.source_id JOIN content m ON m.id=i.metadata_id WHERE i.source_id=?1 AND i.version=?2 AND i.format=?3", params![id.as_str(),artisan_api::SOURCE_INDEX_VERSION,format.as_str()], |row| Ok(SourceIndexHeader {
        format, source: ContentRef { id:id.clone(),bytes:row.get(0)?,lines:row.get(1)? },
        metadata: ContentRef { id:ContentId(row.get(2)?),bytes:row.get(3)?,lines:row.get(4)? },
        version: artisan_api::SOURCE_INDEX_VERSION, leaves:row.get(5)?,
    })).optional().map_err(Into::into)
}
pub(crate) struct PreparedIndex {
    source: ContentRef,
    format: SourceIndexFormat,
    metadata: String,
    leaves: Vec<SourceLeafDescriptor>,
}
pub(crate) fn prepare(
    conn: &Connection,
    id: &ContentId,
    format: SourceIndexFormat,
) -> Result<Option<PreparedIndex>> {
    let Some(source) = conn
        .query_row(
            "SELECT bytes,lines FROM content WHERE id=?1",
            [id.as_str()],
            |row| {
                Ok(ContentRef {
                    id: id.clone(),
                    bytes: row.get(0)?,
                    lines: row.get(1)?,
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let text = crate::content::resolve(conn, &TextContent::Stored(source.clone()))?;
    ensure!(
        ContentRef::from_text(&text) == source,
        "source identity does not match immutable bytes"
    );
    let index = match format {
        SourceIndexFormat::Markdown => Some(artisan_markdown_index::MarkdownIndex::build(&text)?),
        SourceIndexFormat::Literal => None,
    };
    // Literal scanning stores offsets only, never parser events derived from syntax.
    let mut literal_start = Some(0);
    let literal_ranges = std::iter::from_fn(|| {
        let start = literal_start?;
        if let Some(relative_end) = text[start..].find('\n') {
            let end = start + relative_end + 1;
            literal_start = Some(end);
            Some(start..end)
        } else {
            literal_start = None;
            Some(start..text.len())
        }
    });
    let records: Box<dyn Iterator<Item = artisan_markdown_index::LeafRecord> + '_> =
        if let Some(index) = &index {
            Box::new(
                (0..index.leaves().len())
                    .map(|ordinal| index.record(ordinal).expect("indexed ordinal")),
            )
        } else {
            Box::new(literal_ranges.map(artisan_markdown_index::LeafRecord::literal))
        };
    let mut metadata = String::new();
    let mut leaves = Vec::new();
    for (ordinal, record) in records.enumerate() {
        let json = serde_json::to_string(&record)?;
        ensure!(
            metadata.len().saturating_add(json.len() + 1) <= MAX_METADATA_BYTES,
            "serialized source index exceeds 32 MiB"
        );
        let range = record.leaf.source_range();
        let start = metadata.len() as u64;
        metadata.push_str(&json);
        let end = metadata.len() as u64;
        metadata.push('\n');
        leaves.push(SourceLeafDescriptor {
            ordinal: ordinal as u64,
            source_start: range.start as u64,
            source_end: range.end as u64,
            metadata_start: start,
            metadata_end: end,
        });
    }
    Ok(Some(PreparedIndex {
        source,
        format,
        metadata,
        leaves,
    }))
}
pub(crate) fn persist(
    conn: &Connection,
    prepared: PreparedIndex,
) -> Result<Option<SourceIndexHeader>> {
    if let Some(existing) = header(conn, &prepared.source.id, prepared.format)? {
        return Ok(Some(existing));
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM content WHERE id=?1 AND bytes=?2 AND lines=?3)",
        params![
            prepared.source.id.as_str(),
            prepared.source.bytes,
            prepared.source.lines
        ],
        |row| row.get(0),
    )?;
    // GC may have removed an unowned original while its index worker ran.
    if !exists {
        return Ok(None);
    }
    // Even tiny derived indexes must belong to the same immutable content owner.
    let metadata_ref = crate::content::store_source(conn, &prepared.metadata)?;
    ensure!(
        metadata_ref.id != prepared.source.id,
        "derived index cannot own itself"
    );
    // Preserve ownership and cleanup through the existing dependency trigger.
    conn.execute(
        "INSERT INTO content_dependencies(parent_id,child_id) VALUES(?1,?2) ON CONFLICT DO NOTHING",
        params![prepared.source.id.as_str(), metadata_ref.id.as_str()],
    )?;
    conn.execute(
        "INSERT INTO source_indexes(source_id,version,metadata_id,leaf_count,format) VALUES(?1,?2,?3,?4,?5)",
        params![
            prepared.source.id.as_str(),
            artisan_api::SOURCE_INDEX_VERSION,
            metadata_ref.id.as_str(),
            prepared.leaves.len() as u64, prepared.format.as_str()
        ],
    )?;
    let mut insert=conn.prepare_cached("INSERT INTO source_index_leaves(source_id,version,ordinal,source_start,source_end,metadata_start,metadata_end,format) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")?;
    for leaf in prepared.leaves {
        insert.execute(params![
            prepared.source.id.as_str(),
            artisan_api::SOURCE_INDEX_VERSION,
            leaf.ordinal,
            leaf.source_start,
            leaf.source_end,
            leaf.metadata_start,
            leaf.metadata_end,
            prepared.format.as_str()
        ])?;
    }
    drop(insert);
    header(conn, &prepared.source.id, prepared.format)
}
pub(crate) fn range(
    conn: &Connection,
    id: &ContentId,
    query: SourceIndexQuery,
) -> Result<Option<SourceIndexRange>> {
    let Some(index) = header(conn, id, query.format)? else {
        return Ok(None);
    };
    let limit = query.limit.unwrap_or(artisan_api::SOURCE_INDEX_PAGE_LEAVES);
    ensure!(
        (1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&limit),
        "source index page limit must be 1..64 leaves"
    );
    let start = query
        .offset
        .unwrap_or(index.leaves.saturating_sub(u64::from(limit)));
    ensure!(
        start <= index.leaves,
        "source leaf offset exceeds index length"
    );
    let mut select=conn.prepare_cached("SELECT ordinal,source_start,source_end,metadata_start,metadata_end FROM source_index_leaves WHERE source_id=?1 AND version=?2 AND ordinal>=?3 AND format=?5 ORDER BY ordinal LIMIT ?4")?;
    let leaves = select
        .query_map(
            params![
                id.as_str(),
                index.version,
                start,
                limit,
                query.format.as_str()
            ],
            |row| {
                Ok(SourceLeafDescriptor {
                    ordinal: row.get(0)?,
                    source_start: row.get(1)?,
                    source_end: row.get(2)?,
                    metadata_start: row.get(3)?,
                    metadata_end: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let end = start + leaves.len() as u64;
    ensure!(
        end == start.saturating_add(u64::from(limit)).min(index.leaves),
        "source topology contains a gap"
    );
    ensure!(
        leaves
            .iter()
            .enumerate()
            .all(|(n, leaf)| leaf.ordinal == start + n as u64),
        "source topology ordinals are inconsistent"
    );
    Ok(Some(SourceIndexRange {
        index,
        start,
        end,
        leaves,
    }))
}
