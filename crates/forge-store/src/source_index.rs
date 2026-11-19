use crate::content::ContentError;
use crate::error::StorageError;
use artisan_api::{
    ContentId, ContentRef, SourceIndexFormat, SourceIndexHeader, SourceIndexQuery,
    SourceIndexRange, SourceLeafDescriptor, TextContent,
};
use rusqlite::{Connection, OptionalExtension, params};
pub const MAX_INDEX_BYTES: usize = 32 * 1024 * 1024;

type Result<T, E = SourceIndexError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum SourceIndexError {
    #[error("{source_id} is not a source identity")]
    InvalidIdentity { source_id: ContentId },
    #[error("a page of {limit} leaves is outside 1..64")]
    PageLimit { limit: u32 },
    #[error("leaf offset {offset} is past the {leaves} leaves of source {source_id}")]
    OffsetPastEnd {
        source_id: ContentId,
        offset: u64,
        leaves: u64,
    },
    #[error("the bytes of source {source_id} do not match its identity")]
    IdentityMismatch { source_id: ContentId },
    #[error("index the Markdown of source {source_id}")]
    Markdown {
        source_id: ContentId,
        #[source]
        source: artisan_markdown_index::BuildIndexError,
    },
    #[error("the Markdown index of source {source_id} has no record for leaf {ordinal}")]
    MissingRecord {
        source_id: ContentId,
        ordinal: usize,
    },
    #[error("the serialized index of source {source_id} exceeds 32 MiB")]
    MetadataTooLarge { source_id: ContentId },
    #[error("the derived index of source {source_id} would own itself")]
    SelfOwned { source_id: ContentId },
    #[error("the stored topology of source {source_id} has a gap")]
    TopologyGap { source_id: ContentId },
    #[error("the stored topology ordinals of source {source_id} are inconsistent")]
    TopologyOrdinals { source_id: ContentId },
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for SourceIndexError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

pub fn header(
    conn: &Connection,
    id: &ContentId,
    format: SourceIndexFormat,
) -> Result<Option<SourceIndexHeader>> {
    if !id.is_valid() {
        return Err(SourceIndexError::InvalidIdentity {
            source_id: id.clone(),
        });
    }
    conn.query_row(
        include_str!("sql/sources/index_header.sql"),
        params![
            id.as_str(),
            artisan_api::SOURCE_INDEX_VERSION,
            format.as_str()
        ],
        |row| {
            Ok(SourceIndexHeader {
                format,
                source: ContentRef {
                    id: id.clone(),
                    bytes: row.get(0)?,
                    lines: row.get(1)?,
                },
                metadata: ContentRef {
                    id: ContentId(row.get(2)?),
                    bytes: row.get(3)?,
                    lines: row.get(4)?,
                },
                version: artisan_api::SOURCE_INDEX_VERSION,
                leaves: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
pub struct PreparedIndex {
    source: ContentRef,
    format: SourceIndexFormat,
    metadata: String,
    leaves: Vec<SourceLeafDescriptor>,
}

pub enum Prepared {
    Missing,
    Indexed,
    Built(PreparedIndex),
}

pub fn prepare(conn: &Connection, id: &ContentId, format: SourceIndexFormat) -> Result<Prepared> {
    if header(conn, id, format)?.is_some() {
        return Ok(Prepared::Indexed);
    }
    let Some(source) = conn
        .query_row(include_str!("sql/content/size.sql"), [id.as_str()], |row| {
            Ok(ContentRef {
                id: id.clone(),
                bytes: row.get(0)?,
                lines: row.get(1)?,
            })
        })
        .optional()?
    else {
        return Ok(Prepared::Missing);
    };
    let text = crate::content::resolve(conn, &TextContent::Stored(source.clone()))?;
    if ContentRef::from_text(&text) != source {
        return Err(SourceIndexError::IdentityMismatch {
            source_id: id.clone(),
        });
    }
    let index = match format {
        SourceIndexFormat::Markdown => Some(
            artisan_markdown_index::MarkdownIndex::build(&text).map_err(|source| {
                SourceIndexError::Markdown {
                    source_id: id.clone(),
                    source,
                }
            })?,
        ),
        SourceIndexFormat::Literal => None,
    };
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
    let records: Box<dyn Iterator<Item = Result<artisan_markdown_index::LeafRecord>> + '_> =
        if let Some(index) = &index {
            Box::new((0..index.leaves().len()).map(|ordinal| {
                index
                    .record(ordinal)
                    .ok_or_else(|| SourceIndexError::MissingRecord {
                        source_id: id.clone(),
                        ordinal,
                    })
            }))
        } else {
            Box::new(
                literal_ranges.map(|range| Ok(artisan_markdown_index::LeafRecord::literal(range))),
            )
        };
    let mut metadata = String::new();
    let mut leaves = Vec::new();
    for (ordinal, record) in records.enumerate() {
        let record = record?;
        let json = crate::read::to_body(&record)?;
        if metadata.len().saturating_add(json.len() + 1) > MAX_INDEX_BYTES {
            return Err(SourceIndexError::MetadataTooLarge {
                source_id: id.clone(),
            });
        }
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
    Ok(Prepared::Built(PreparedIndex {
        source,
        format,
        metadata,
        leaves,
    }))
}
pub const LEAVES_PER_WRITE: usize = 2048;

pub enum Staging {
    Published,
    Gone,
    Staged,
}

fn source_present(conn: &Connection, source: &ContentRef) -> Result<bool> {
    Ok(conn.query_row(
        include_str!("sql/sources/present.sql"),
        params![source.id.as_str(), source.bytes, source.lines],
        |row| row.get(0),
    )?)
}

pub fn stage(conn: &Connection, prepared: &PreparedIndex, batch: usize) -> Result<Staging> {
    if header(conn, &prepared.source.id, prepared.format)?.is_some() {
        return Ok(Staging::Published);
    }
    if !source_present(conn, &prepared.source)? {
        return Ok(Staging::Gone);
    }
    let mut insert = conn.prepare_cached(include_str!("sql/sources/stage_leaf.sql"))?;
    for leaf in prepared
        .leaves
        .iter()
        .skip(batch * LEAVES_PER_WRITE)
        .take(LEAVES_PER_WRITE)
    {
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
    Ok(Staging::Staged)
}

impl PreparedIndex {
    #[must_use]
    pub fn batches(&self) -> usize {
        self.leaves.len().div_ceil(LEAVES_PER_WRITE)
    }
}

pub fn publish(conn: &Connection, prepared: &PreparedIndex) -> Result<Option<SourceIndexHeader>> {
    if let Some(existing) = header(conn, &prepared.source.id, prepared.format)? {
        return Ok(Some(existing));
    }
    if !source_present(conn, &prepared.source)? {
        return Ok(None);
    }
    let metadata_ref = crate::content::store_source(conn, &prepared.metadata)?;
    if metadata_ref.id == prepared.source.id {
        return Err(SourceIndexError::SelfOwned {
            source_id: prepared.source.id.clone(),
        });
    }
    conn.execute(
        include_str!("sql/content/insert_dependency.sql"),
        params![prepared.source.id.as_str(), metadata_ref.id.as_str()],
    )?;
    conn.execute(
        include_str!("sql/sources/insert_index.sql"),
        params![
            prepared.source.id.as_str(),
            artisan_api::SOURCE_INDEX_VERSION,
            metadata_ref.id.as_str(),
            prepared.leaves.len() as u64,
            prepared.format.as_str()
        ],
    )?;
    header(conn, &prepared.source.id, prepared.format)
}
pub fn range(
    conn: &Connection,
    id: &ContentId,
    query: &SourceIndexQuery,
) -> Result<Option<SourceIndexRange>> {
    let Some(index) = header(conn, id, query.format)? else {
        return Ok(None);
    };
    let limit = query.limit.unwrap_or(artisan_api::SOURCE_INDEX_PAGE_LEAVES);
    if !(1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&limit) {
        return Err(SourceIndexError::PageLimit { limit });
    }
    let start = query
        .offset
        .unwrap_or_else(|| index.leaves.saturating_sub(u64::from(limit)));
    if start > index.leaves {
        return Err(SourceIndexError::OffsetPastEnd {
            source_id: id.clone(),
            offset: start,
            leaves: index.leaves,
        });
    }
    let mut select = conn.prepare_cached(include_str!("sql/sources/leaves.sql"))?;
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
    if end != start.saturating_add(u64::from(limit)).min(index.leaves) {
        return Err(SourceIndexError::TopologyGap {
            source_id: id.clone(),
        });
    }
    if !leaves
        .iter()
        .enumerate()
        .all(|(n, leaf)| leaf.ordinal == start + n as u64)
    {
        return Err(SourceIndexError::TopologyOrdinals {
            source_id: id.clone(),
        });
    }
    Ok(Some(SourceIndexRange {
        index,
        start,
        end,
        leaves,
    }))
}
